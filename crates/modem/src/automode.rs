//! Automode for a far end that does not do V.8: what V.32bis Annex A has a
//! modem do when the answer is not ANSam.
//!
//! V.8 settles a modulation between two modems that both speak it. A modem
//! from before V.8 -- a V.22bis, a V.21, a Bell 212A or 103 -- answers with
//! the plain answer tone of V.25, or with no 2100 Hz at all, and then says
//! the one thing its own Recommendation has it say. So the two ends find each
//! other by listening, as V.32bis Annex A lays out:
//!
//! - The calling modem listens for the answerer's first signal: 2250 Hz is
//!   V.22 and V.22bis's unscrambled binary 1 (a phase step of 270 degrees a
//!   symbol at 600 baud on the 2400 Hz carrier), 2225 Hz is a Bell answer
//!   (212A and 103 alike, with no V.25 tone in front), 1650 Hz is V.21's
//!   answering mark. V.32's AC is the V.32 start-up's own to hear.
//! - The answering modem cannot listen for anything: each of those callers is
//!   silent until it hears its own answer. So it offers them in turn -- AC,
//!   then unscrambled ones, then 2225 Hz, then V.21's mark -- and keeps the
//!   first one the caller answers.
//!
//! 2225 and 2250 Hz are twenty-five hertz apart, so the ear works in blocks
//! of 40 ms: a Goertzel filter over 40 ms has its first null exactly 25 Hz
//! from its centre, and each tone falls in the other's null.

/// A tone the calling modem knows a pre-V.8 answerer by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// 2250 Hz: V.22 or V.22bis, unscrambled binary 1 (6.3.1.1.2 a).
    V22bis,
    /// 2225 Hz: Bell 212A or Bell 103, the answer tone that is Bell 103's mark.
    Bell,
    /// 1650 Hz: V.21's answering channel at mark.
    V21,
}

impl Answer {
    /// The `+MS` carrier that answers it.
    pub fn carrier(self) -> &'static str {
        match self {
            Self::V22bis => "V22B",
            Self::Bell => "B212",
            Self::V21 => "V21",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::V22bis => "unscrambled ones at 2250 Hz: V.22bis",
            Self::Bell => "2225 Hz: a Bell answer",
            Self::V21 => "1650 Hz: V.21's answering mark",
        }
    }

    fn hertz(self) -> f64 {
        match self {
            Self::V22bis => 2250.0,
            Self::Bell => 2225.0,
            Self::V21 => 1650.0,
        }
    }
}

const ANSWERS: [Answer; 3] = [Answer::V22bis, Answer::Bell, Answer::V21];

/// One block's length in seconds: see the module header.
const BLOCK: f64 = 0.040;
/// The share of a block's energy one tone must hold to be that tone. A pure
/// tone holds all of it; V.8 bis's dual-tone signals, whose responding pair
/// has 2225 Hz in it, hold half.
const SHARE: f64 = 0.7;
/// Blocks in a row a tone must hold for: 320 ms.
const BLOCKS_HELD: usize = 8;
/// Quieter than this, a block is not judged at all.
const QUIET: f64 = 1e-6;

/// The calling modem's ear for a pre-V.8 answer.
#[derive(Debug)]
pub struct Ear {
    n: usize,
    coefficients: [f64; 3],
    state: [(f64, f64); 3],
    energy: f64,
    count: usize,
    run: [usize; 3],
    heard: Option<Answer>,
}

impl Ear {
    pub fn new(fs: f64) -> Self {
        let n = (BLOCK * fs).round() as usize;
        let coefficients = ANSWERS.map(|a| 2.0 * (std::f64::consts::TAU * a.hertz() / fs).cos());
        Self { n, coefficients, state: [(0.0, 0.0); 3], energy: 0.0, count: 0, run: [0; 3], heard: None }
    }

    /// One sample from the line. Some once a tone has held long enough, and
    /// from then on.
    pub fn feed(&mut self, x: f64) -> Option<Answer> {
        if self.heard.is_some() {
            return self.heard;
        }
        for (s, c) in self.state.iter_mut().zip(self.coefficients) {
            let next = x + c * s.0 - s.1;
            *s = (next, s.0);
        }
        self.energy += x * x;
        self.count += 1;
        if self.count < self.n {
            return None;
        }
        let n = self.n as f64;
        for (i, ((s1, s2), c)) in self.state.iter().zip(self.coefficients).enumerate() {
            let power = s1 * s1 + s2 * s2 - c * s1 * s2;
            // A pure tone of amplitude A: power (A n / 2)^2, energy A^2 n / 2.
            let share = if self.energy / n > QUIET { 2.0 * power / (n * self.energy) } else { 0.0 };
            self.run[i] = if share > SHARE { self.run[i] + 1 } else { 0 };
            if self.run[i] >= BLOCKS_HELD {
                self.heard = Some(ANSWERS[i]);
            }
        }
        self.state = [(0.0, 0.0); 3];
        self.energy = 0.0;
        self.count = 0;
        self.heard
    }
}

/// What the answering modem offers, in the order it offers them.
pub const OFFERS: [&str; 4] = ["V32B", "V22B", "B212", "V21"];
/// How long each offer is held for an answer, in seconds. A V.22bis caller
/// answers unscrambled ones after 155 ms of them; a Bell caller answers 2225
/// Hz within a second; V.32's AA comes within a second of AC.
pub const OFFER_FOR: f64 = 3.0;

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 8000.0;

    fn hears(hz: f64, seconds: f64) -> Option<Answer> {
        let mut ear = Ear::new(FS);
        let mut heard = None;
        for k in 0..(seconds * FS) as usize {
            heard = ear.feed(0.3 * (std::f64::consts::TAU * hz * k as f64 / FS).sin());
        }
        heard
    }

    #[test]
    fn each_answer_is_heard_as_itself_and_no_other() {
        assert_eq!(hears(2250.0, 0.5), Some(Answer::V22bis));
        assert_eq!(hears(2225.0, 0.5), Some(Answer::Bell));
        assert_eq!(hears(1650.0, 0.5), Some(Answer::V21));
        assert_eq!(hears(2100.0, 3.0), None, "ANS is not an answer of these");
        assert_eq!(hears(1800.0, 3.0), None);
        assert_eq!(hears(2225.0, 0.2), None, "too short to believe");
    }

    #[test]
    fn a_pair_with_2225_in_it_is_not_a_bell_answer() {
        // V.8 bis's responding stations open with 1529 and 2225 Hz together.
        let mut ear = Ear::new(FS);
        let mut heard = None;
        for k in 0..FS as usize {
            let t = k as f64 / FS;
            let x = 0.2 * (std::f64::consts::TAU * 1529.0 * t).sin() + 0.2 * (std::f64::consts::TAU * 2225.0 * t).sin();
            heard = ear.feed(x);
        }
        assert_eq!(heard, None);
    }
}
