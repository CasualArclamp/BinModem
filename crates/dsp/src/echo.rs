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
//!
//! And on a sound card neither stays where it was found. A cable looped from
//! a sound card's output to its input crosses two clocks a few parts per
//! million apart, so what comes back arrives a little later, or a little
//! earlier, every second; and now and then the card drops a sample, repeats
//! one or runs dry, and moves all of it at once. A canceller that learned the
//! echo during training is, a few seconds into the data, cancelling where it
//! used to be. [`EchoCanceller::follow_drift`] is for that.

use std::collections::VecDeque;
use std::f64::consts::PI;
use std::sync::OnceLock;

/// Guards the division when the line is silent.
const FLOOR: f64 = 1.0e-9;

/// How quickly the return-loss meters follow the signal. About a millisecond
/// at the rates used here, which is long enough to average a symbol and short
/// enough to follow a signal starting.
const POWER_TRACK: f64 = 0.01;

/// The first lag a run of taps may start at and follow drift. Runs that
/// start before it stay where training put them (see
/// [`Segment::first_retimed`]).
///
/// The lags under it are the hybrid's, inside a millisecond, and nothing
/// that drifts can arrive that soon: drift is a sound card's two clocks
/// disagreeing, and its buffering alone is longer than this. They are also
/// the lags the interpolator cannot serve, because it reads [`HALF`] samples
/// either side of the point it is asked for and the newest of those would be
/// in the future.
const FIRST_RETIMED: usize = 17;

/// The interpolator reads this many samples either side of the point it
/// interpolates: thirty-two taps.
const HALF: usize = 16;

/// Fractions of a sample the interpolator is tabulated at. Rounding to the
/// nearest of them is at worst 1/2048 of a sample out, which at the top of
/// the band is 64 dB down: far below anything the delay loop can hold.
const PHASES: usize = 1024;

/// Kaiser's window parameter for the interpolator. Measured over 300 to
/// 3400 Hz at every fraction of a sample, 8 leaves the worst error 82 dB down,
/// and 6 would leave it at 64. The band above 3400 Hz, where a wider window
/// would do worse, carries nothing a modem sends.
const KAISER: f64 = 8.0;

/// The delay loop's proportional and integral gains, per sample, at 16 kHz.
///
/// All three pairs are critically damped, and the loop goes through them in
/// turn. The far end is noise to this loop, as loud as the echo on a cable,
/// and a wide loop lets more of it in: what it leaves under the far end is
/// about twice the loop bandwidth over the sample rate, three times over for
/// how narrow the band is. So 8 Hz pulls in from training's delay and a
/// drift of nothing in half a second, but settles only 25 dB under the far
/// end; 1.5 Hz settles at 32, and 0.25 Hz at 40.
///
/// The last is not luxury. What is left over a second is the delay's error
/// over that second, and a narrow loop's error changes slowly, so a minute
/// holds a few seconds of it at three times its usual size: at 1.5 Hz those
/// reached 25 dB under the far end, and at 0.5 Hz 28. Nor can the loop go
/// there at once. Each gear hands the next a rate only as good as its own
/// noise allows, tens of ppm from 8 Hz and a few from 1.5 Hz, and a narrow
/// loop takes seconds to work a wrong rate off: straight from 8 Hz to
/// 0.5 Hz, a 100 ppm cable spent its fourth second 21 dB under.
const FAST: (f64, f64) = (1.6e-3, 6.4e-7);
const SLOW: (f64, f64) = (3.0e-4, 2.25e-8);
const SLOWEST: (f64, f64) = (5.0e-5, 6.25e-10);

/// Updates made at 8 Hz once switched on, half a second's worth, and then at
/// 1.5 Hz, three seconds'.
///
/// The fast gear may not run at all: it is only for a line that is still
/// ours (see [`OURS`]), and a far end already talking at the end of training
/// leaves the slow one to pull the rate in from nothing. A second and a half
/// of that was too little at 100 ppm either way: what it left the slowest
/// gear to work off put the fourth second 19 and 20 dB under the far end,
/// where three seconds' puts it at 27 and 29.
const FAST_AT_FIRST: u32 = 8_000;
const SLOW_AT_FIRST: u32 = 48_000;

/// What is left may be this much of the echo estimate for the line still to
/// count as ours, with nothing on it but our own echo and what the canceller
/// leaves of it: 10 dB under. A far end talking is as loud as the echo on a
/// cable, and seldom less than 10 dB under it anywhere.
const OURS: f64 = 0.1;

/// Updates made after a jump with the delay at 1.5 Hz, 200 ms.
///
/// Only the delay: a slip moves the echo and not the clocks, so the rate is
/// left learning as slowly as it was. And not at 8 Hz, whose own noise is
/// enough to put a 50 ms window over 23 dB under the far end. A slip moves
/// the echo by whole samples, which the search finds exactly, and what this
/// is for is only the little the delay may have been out when it was held.
const REALIGN_AFTER_JUMP: u32 = 3_200;

/// The two meters of what is left, as fractions per sample: 5 ms and 125 ms.
const QUICK: f64 = 1.0 / 80.0;
const STEADY: f64 = 1.0 / 2_000.0;

/// Samples spent measuring before the loop may act, whenever it starts: one
/// steady time constant, so that the steady meter starts from a mean of what
/// is left rather than from nothing.
const WARM: u32 = 2_000;

/// How far the quick meter may rise over the steady one before the loop is
/// held. A slipped sample on a cable adds about half the echo's power to what
/// is left, which with the far end as loud as the echo is well over this.
const WORSE: f64 = 1.25;

/// How long what is left must stay risen for the hold to be believed:
/// 10 ms. Less, and it was the far end louder for a moment; a hold on every
/// such moment, searched, held the loop still for a quarter of the time.
const HELD_BEFORE_SEARCH: usize = 160;

/// What a jump search looks at: the first 30 ms of a believed hold. The hold
/// began after what was left rose, which was after the fault, so all of it is
/// the echo where it has gone to.
const SEARCH_WINDOW: usize = 480;

/// Furthest a jump is looked for, either way: 30 ms, which covers a sound
/// card running dry for a buffer or two.
const SEARCH_REACH: usize = 480;

/// How much better a shift must explain what arrives before it is believed.
const BETTER: f64 = 0.8;

/// Weakest echo estimate worth following, against what arrives: 40 dB down.
/// A four-wire line has nothing to follow, and nor has a VoIP call, whose
/// network returns nothing measurable of what we send.
const AUDIBLE: f64 = 1.0e-4;

/// Weakest echo estimate the loop can follow, against what is left once it
/// is cancelled: 15 dB down.
///
/// The loop reads the delay through the far end, and how badly depends on
/// how loud the echo is against it. At 15 dB under the far end the reading
/// wanders by a fifth of a sample at 1.5 Hz, which it can still be pulled
/// back from. Much further down it is noise with nothing to pull it
/// anywhere. Retiming once reached the band-limited tail of a hybrid's own
/// taps, 30 dB under the far end on a line that did not drift at all, and
/// following that had the delay walk a thousand samples in twenty seconds.
const HEARABLE: f64 = 0.03;

/// Fastest drift believed, in samples per sample: a thousand parts per
/// million, ten times what the worst pair of sound-card clocks manage. A
/// loop that has lost what it was following reads noise, and its rate walks
/// with it; this is where the walk stops.
const MOST_DRIFT: f64 = 1.0e-3;

/// How far under its own mean the echo estimate may fall with the loop still
/// learning from it: a quarter. Below that we are not sending, or not much,
/// and what is left is not ours to explain.
const SENDING: f64 = 0.25;

/// The retimed taps' output, kept for the interpolator: 32 taps, one more
/// either side for the two half-sample points the loop's slope is taken
/// between, and one for rounding to the next sample.
const RING: usize = 2 * HALF + 2;

/// Reference kept beyond the taps while following, for the jump search: a
/// window and its reach either side, the interpolator's width, and a jump's
/// worth of headroom so that a jump is never taken to a delay whose samples
/// have been let go.
const HISTORY_SPARE: usize = SEARCH_WINDOW + 2 * SEARCH_REACH + 2 * HALF + 2;

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

    /// The first of these taps that follows drift, as an index into them:
    /// all of them or none.
    ///
    /// A run is one filter, learned whole, and least mean squares on a
    /// band-limited reference spreads what it learns across all of it. Split
    /// one and retime half, and the two halves pull apart as the delay
    /// moves. The near run of a cable canceller, which has nothing to model,
    /// learned misadjustment in two parts 36 and 32 dB down that largely
    /// cancelled each other; retimed from lag 17 on, they stopped cancelling,
    /// and cost 2 dB on a 20 ppm cable in a minute. So the near run, which
    /// starts at the hybrid, stays as trained, and a run that starts where a
    /// sound card's echo can be follows the drift whole.
    fn first_retimed(&self) -> usize {
        if self.offset >= FIRST_RETIMED {
            0
        } else {
            self.taps.len()
        }
    }

    /// The first retimed tap that can be read `shift` samples from its own
    /// lag without reading the future. The ones before it have been carried
    /// inside the interpolator's reach by an echo arriving earlier than it
    /// was learned, and read nothing.
    fn first_readable(&self, shift: i64) -> usize {
        let past = (-shift - self.offset as i64).max(0) as usize;
        self.first_retimed().max(past).min(self.taps.len())
    }

    /// The taps that stay where training put them, applied to the reference.
    fn unretimed_echo(&self, history: &VecDeque<f64>) -> f64 {
        (0..self.first_retimed())
            .map(|i| self.taps[i] * history[self.offset + i])
            .sum()
    }

    /// The retimed taps applied to the reference as it was `shift` samples
    /// behind their own lags, `from` samples ago.
    fn retimed_echo(&self, history: &VecDeque<f64>, from: usize, shift: i64) -> f64 {
        let start = self.offset as i64 + shift + from as i64;
        (self.first_readable(shift)..self.taps.len())
            .map(|i| self.taps[i] * history[(start + i as i64) as usize])
            .sum()
    }
}

/// The interpolator, one row of 32 taps for each of [`PHASES`] fractions.
///
/// Row `p` reads the point `p / PHASES` of a sample further back than the
/// sample its sixteenth tap sits on. Each row is normalised to unit gain,
/// which keeps the level exactly where it was whatever fraction is asked for.
fn interpolator() -> &'static [[f64; 2 * HALF]] {
    static TABLE: OnceLock<Vec<[f64; 2 * HALF]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..PHASES)
            .map(|p| kaiser_sinc(p as f64 / PHASES as f64))
            .collect()
    })
}

fn kaiser_sinc(fraction: f64) -> [f64; 2 * HALF] {
    let mut row = [0.0; 2 * HALF];
    if fraction == 0.0 {
        // Exactly the sample, and nothing of its neighbours: a delay that
        // has not moved reads the reference exactly as the plain taps do.
        row[HALF - 1] = 1.0;
        return row;
    }
    let edge = bessel_i0(KAISER);
    for (j, weight) in row.iter_mut().enumerate() {
        // How far this tap's sample is from the point, in samples.
        let t = j as f64 - (HALF - 1) as f64 - fraction;
        let r = t / HALF as f64;
        let window = bessel_i0(KAISER * (1.0 - r * r).max(0.0).sqrt()) / edge;
        *weight = (PI * t).sin() / (PI * t) * window;
    }
    let sum: f64 = row.iter().sum();
    row.iter_mut().for_each(|w| *w /= sum);
    row
}

/// Zeroth-order modified Bessel function of the first kind, for Kaiser's
/// window. The series converges fast for the arguments a window produces.
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

/// Where in the ring of retimed output to read a delay `fraction` samples
/// past the whole part, which lies within a sample either way: the ring
/// entry the interpolator starts at, and the row of the table to use.
fn place(fraction: f64) -> (usize, usize) {
    let whole = fraction.floor();
    let mut phase = ((fraction - whole) * PHASES as f64).round() as usize;
    let mut first = (whole as i64 + 1) as usize;
    if phase == PHASES {
        phase = 0;
        first += 1;
    }
    (first, phase)
}

/// The retimed output `fraction` of a sample past the whole delay.
fn interpolate(ring: &VecDeque<f64>, fraction: f64) -> f64 {
    let (first, phase) = place(fraction);
    interpolator()[phase]
        .iter()
        .enumerate()
        .map(|(j, w)| w * ring[first + j])
        .sum()
}

/// Everything the canceller needs to follow an echo that moves.
///
/// The echo moves as a whole: every reflection behind the sound card is
/// behind the same two clocks. So the retimed taps share one delay, a whole
/// number of samples and a fraction, and one loop moves it.
///
/// Sharing one delay is also what makes it cheap. With the taps still, the
/// retimed part is one filter, and a filter and a delay commute: rather than
/// interpolate the reference at every tap, the canceller runs the taps over
/// the reference as it stands and interpolates their output, once. The ring
/// holds that output, computed as far ahead as the taps' own lags allow,
/// which is what the interpolator's look-ahead is paid from.
#[derive(Debug, Clone)]
struct Drift {
    /// The delay beyond the taps' own lags: whole samples, and a fraction
    /// between minus and plus a half.
    whole: i64,
    fraction: f64,
    /// Samples of delay gained per sample: the loop's integrator, and the
    /// difference between the two clocks.
    rate: f64,
    /// Updates still to make at 8 Hz and at 1.5 Hz before the loop settles
    /// at 0.25 Hz, and still to make with the delay alone at 1.5 Hz after a
    /// jump.
    fast: u32,
    slow: u32,
    realigning: u32,
    /// Samples still to spend measuring before acting, and what was left
    /// over them, for the steady meter's first value.
    warming: u32,
    warm_sum: f64,
    /// What is left, over 5 ms and over 125 ms. The steady one stands still
    /// while held, so that it still says what normal was.
    quick: f64,
    steady: f64,
    /// The echo estimate's slope along the delay, over 5 ms: what the loop's
    /// error is divided by, so that it reads in samples.
    slope: f64,
    /// The echo estimate over 125 ms and over 5 ms, and what arrives over
    /// 125 ms, for whether there is anything to follow and whether we are
    /// sending it now.
    echo: f64,
    echo_now: f64,
    heard: f64,
    /// The delay and rate as they were the last time what was left was no
    /// worse than steady, and how long ago that was: what a hold goes back
    /// to, and a jump is measured from.
    before: (i64, f64, f64),
    since_before: u32,
    /// Samples held, while held.
    held: Option<usize>,
    jumps: u32,
    /// The retimed taps' output, newest first. See [`RING`].
    ring: VecDeque<f64>,
    /// Whether the ring was computed from other taps or another whole delay
    /// than the ones there are now, and wants computing again.
    stale: bool,
    /// What the retimed taps have had to explain: what arrived, less what
    /// the unretimed ones took out. Newest first, a search window's worth.
    recent: VecDeque<f64>,
    /// Each retimed tap's own interpolated reference, while the taps learn.
    scratch: Vec<f64>,
}

impl Drift {
    fn new() -> Self {
        Self {
            whole: 0,
            fraction: 0.0,
            rate: 0.0,
            fast: FAST_AT_FIRST,
            slow: SLOW_AT_FIRST,
            realigning: 0,
            warming: WARM,
            warm_sum: 0.0,
            quick: 0.0,
            steady: 0.0,
            slope: 0.0,
            echo: 0.0,
            echo_now: 0.0,
            heard: 0.0,
            before: (0, 0.0, 0.0),
            since_before: 0,
            held: None,
            jumps: 0,
            ring: VecDeque::from(vec![0.0; RING]),
            stale: true,
            recent: VecDeque::from(vec![0.0; SEARCH_WINDOW]),
            scratch: Vec::new(),
        }
    }

    /// How far the retimed taps read behind their own lags, in whole samples
    /// and less the interpolator's look-ahead.
    fn shift(&self) -> i64 {
        self.whole - HALF as i64
    }

    /// Take a whole sample off the fraction, or put one on, whenever it
    /// passes a half.
    fn wrap(&mut self) {
        let was = self.whole;
        while self.fraction >= 0.5 {
            self.fraction -= 1.0;
            self.whole += 1;
        }
        while self.fraction < -0.5 {
            self.fraction += 1.0;
            self.whole -= 1;
        }
        if self.whole != was {
            self.stale = true;
        }
    }

    fn remember(&mut self) {
        self.before = (self.whole, self.fraction, self.rate);
        self.since_before = 0;
    }

    /// Back to the delay as it was the last time nothing was wrong, carried
    /// forward at the rate it had then.
    fn go_back(&mut self) {
        let (whole, fraction, rate) = self.before;
        let was = self.whole;
        self.whole = whole;
        self.fraction = fraction + rate * f64::from(self.since_before);
        self.rate = rate;
        if self.whole != was {
            self.stale = true;
        }
        self.wrap();
        self.remember();
    }

    /// Take in one sample's worth of what the canceller did, and move the
    /// delay if there is reason to. True when it is time to look for a jump.
    fn observe(&mut self, left: f64, estimate: f64, slope: f64, received: f64) -> bool {
        let e2 = left * left;
        self.heard += STEADY * (received * received - self.heard);
        self.echo += STEADY * (estimate * estimate - self.echo);
        self.echo_now += QUICK * (estimate * estimate - self.echo_now);
        self.slope += QUICK * (slope * slope - self.slope);
        self.quick += QUICK * (e2 - self.quick);
        self.since_before = self.since_before.saturating_add(1);
        self.coast();

        if self.warming > 0 {
            self.warming -= 1;
            self.warm_sum += e2;
            if self.warming == 0 {
                self.steady = self.warm_sum / f64::from(WARM);
                self.quick = self.steady;
                self.remember();
            }
            return false;
        }

        let worse = self.quick > WORSE * self.steady;
        match self.held {
            Some(held) if held >= HELD_BEFORE_SEARCH => {
                // Believed, and gathering the window, which began with the
                // hold: what was left rose after the fault, so everything
                // since is after it.
                self.held = Some(held + 1);
                return held + 1 >= SEARCH_WINDOW;
            }
            Some(held) if worse => {
                self.held = Some(held + 1);
                if held + 1 == HELD_BEFORE_SEARCH {
                    // Something moved faster than any clock drifts, and has
                    // stayed moved. Whatever the loop did while what was left
                    // rose, it did on a signal that had stopped meaning what
                    // it meant, so it is undone.
                    self.go_back();
                }
                return false;
            }
            // What is left has gone back to normal before the hold was
            // believed: the far end, louder for a moment, and nothing moved.
            Some(_) => self.held = None,
            None => {}
        }

        self.steady += STEADY * (e2 - self.steady);
        if self.quick <= self.steady {
            self.remember();
        }
        // Whether there is an echo here the loop can hear. Against what is
        // heard, there has to be one at all; against what is left, it has to
        // be loud enough that the loop's reading of it is not mostly the far
        // end, or the loop wanders off on noise with nothing to pull it back.
        let followable = self.echo >= AUDIBLE * self.heard && self.echo >= HEARABLE * self.steady;
        if followable && worse {
            self.held = Some(0);
            return false;
        }
        if followable && self.echo_now >= SENDING * self.echo {
            // What is left is the far end, plus the echo's slope times however
            // far out the delay is. Its correlation with the slope, divided by
            // the slope's own power, is that distance in samples.
            let error = -left * slope / (self.slope + FLOOR);
            // 8 Hz only while the line is still ours: once the far end talks
            // over the echo, a loop that wide takes enough of it in to put
            // the rate tens of ppm out, and the delay carries whatever rate
            // it has across any silence. On a 5 ppm cable the answering
            // modem's R1 was read as 5.2 ppm alone on the line; the calling
            // modem's S over the rest of the fast gear made it 75, and the
            // 2 s the answering modem is silent for after R1 put its echo 2.3
            // samples out.
            if self.steady > OURS * self.echo {
                self.fast = 0;
            }
            let (mut kp, ki) = if self.fast > 0 {
                self.fast -= 1;
                FAST
            } else if self.slow > 0 {
                self.slow -= 1;
                SLOW
            } else {
                SLOWEST
            };
            if self.realigning > 0 {
                self.realigning -= 1;
                kp = kp.max(SLOW.0);
            }
            self.rate = (self.rate + ki * error).clamp(-MOST_DRIFT, MOST_DRIFT);
            self.fraction += kp * error;
            self.wrap();
        }
        false
    }

    /// Move the delay on by the rate it has. The clocks do not stop for
    /// anything the loop is waiting for: not a hold, not the far end going
    /// quiet, and not the taps learning again.
    fn coast(&mut self) {
        self.fraction += self.rate;
        self.wrap();
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
    /// Present only while following drift. Without it the canceller is
    /// exactly what it was before it could.
    drift: Option<Box<Drift>>,
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
            drift: None,
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
    ///
    /// While following drift, `delay` is where the reflection is now, as a
    /// finder measures it, and the run goes where the retimed taps will meet
    /// it: the whole samples the echo has drifted by come off.
    pub fn watch_far_echo(&mut self, delay: usize, taps: usize) {
        let delay = match &self.drift {
            Some(drift) => (delay as i64 - drift.whole).max(self.near.end() as i64) as usize,
            None => delay,
        };
        let mut far = Segment::new(delay, taps);
        let reach = self.near.end().max(far.end()) + 1;
        match &self.drift {
            None => self.history.resize(reach, 0.0),
            Some(drift) => {
                let want = reach + drift.whole.max(0) as usize + HISTORY_SPARE;
                if self.history.len() < want {
                    self.history.resize(want, 0.0);
                }
            }
        }
        far.recount(&self.history);
        self.far = Some(far);
        if let Some(drift) = self.drift.as_mut() {
            drift.stale = true;
        }
    }

    /// Follow an echo that drifts, or stop.
    ///
    /// For the line a sound card makes, where both directions cross two
    /// clocks that disagree by a few parts per million and the card slips a
    /// sample, or a buffer, now and then. Off, which is how it starts, the
    /// canceller is exactly what it was without it.
    ///
    /// Following, the far run of taps reads the reference through one delay
    /// the canceller measures for itself: a fraction of a sample from a
    /// 32-tap interpolator, and whole samples when the fraction passes a
    /// half. Any run that starts 17 samples back or more does; the near run,
    /// which starts at the hybrid, does not, since nothing that drifts can
    /// arrive that soon. A second-order loop moves the delay, from the
    /// correlation of what is left with the echo estimate's slope along it.
    /// That is one number to learn where least mean squares would have
    /// hundreds, which is what lets it learn through a far end as loud as
    /// the echo: the taps cannot, and are left as training made them.
    ///
    /// A sample dropped or repeated, or a buffer of silence, moves the echo
    /// faster than any clock drifts. What is left jumps, the loop is held,
    /// and once the jump has lasted a search over whole samples either way
    /// finds where the echo went. A search that finds nothing better takes
    /// what is left as the new normal and lets go, which is what a far end
    /// starting to talk, or changing its level, looks like.
    ///
    /// While the taps are adapting the loop learns nothing, and the delay
    /// goes on at the rate it had, which keeps the echo still under taps
    /// learning it again. Once they stop it carries on from there.
    ///
    /// Switching off forgets the delay, and a canceller that had followed one
    /// reads the reference as it arrives again, keeping only as much of it as
    /// the plain canceller keeps.
    pub fn follow_drift(&mut self, on: bool) {
        if on == self.drift.is_some() {
            return;
        }
        if on {
            self.drift = Some(Box::new(Drift::new()));
            self.fit_history();
        } else {
            self.drift = None;
            self.history.truncate(self.span() + 1);
        }
    }

    /// How fast the echo is drifting, in parts per million: positive when it
    /// is coming back later and later, which is the input's clock running
    /// fast against the output's. Zero when not following.
    pub fn drift_ppm(&self) -> f64 {
        self.drift.as_ref().map_or(0.0, |d| d.rate * 1.0e6)
    }

    /// How many times the echo has been found to have jumped: a slip or an
    /// underrun each, on a cable. Zero when not following.
    pub fn jumps(&self) -> u32 {
        self.drift.as_ref().map_or(0, |d| d.jumps)
    }

    /// Whether the delay is being held while something on the line changes
    /// faster than drift can.
    pub fn is_holding(&self) -> bool {
        self.drift.as_ref().is_some_and(|d| d.held.is_some())
    }

    /// Keep enough of the reference for the taps at the delay they have got
    /// to, and for a search from there.
    fn fit_history(&mut self) {
        if let Some(drift) = &self.drift {
            let want = self.span() + 1 + drift.whole.max(0) as usize + HISTORY_SPARE;
            if self.history.len() < want {
                self.history.resize(want, 0.0);
            }
        }
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
        if let Some(mut drift) = self.drift.take() {
            let left = if self.adapting {
                self.learn_retimed(&mut drift, transmitted, received)
            } else {
                self.follow(&mut drift, transmitted, received)
            };
            self.drift = Some(drift);
            self.fit_history();
            return left;
        }

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

    /// Take in what went out, as the plain canceller does, keeping each
    /// run's energy exact for whenever it is needed again.
    fn take_in(&mut self, transmitted: f64) {
        self.history.pop_back();
        self.history.push_front(transmitted);
        self.near.shift(&self.history);
        if let Some(far) = self.far.as_mut() {
            far.shift(&self.history);
        }
    }

    fn meter(&mut self, received: f64, left: f64) {
        self.heard += POWER_TRACK * (received * received - self.heard);
        self.residue += POWER_TRACK * (left * left - self.residue);
    }

    /// The taps that stay where training put them, applied to the reference.
    fn unretimed_echo(&self) -> f64 {
        self.near.unretimed_echo(&self.history)
            + self.far.as_ref().map_or(0.0, |f| f.unretimed_echo(&self.history))
    }

    /// The retimed taps' output `from` samples ago, reading `shift` behind
    /// their own lags.
    fn retimed_output(&self, from: usize, shift: i64) -> f64 {
        self.near.retimed_echo(&self.history, from, shift)
            + self
                .far
                .as_ref()
                .map_or(0.0, |f| f.retimed_echo(&self.history, from, shift))
    }

    /// One sample following drift, with the taps held still.
    fn follow(&mut self, drift: &mut Drift, transmitted: f64, received: f64) -> f64 {
        self.take_in(transmitted);
        let shift = drift.shift();
        if drift.stale {
            for from in 0..RING {
                drift.ring[from] = self.retimed_output(from, shift);
            }
            drift.stale = false;
        } else {
            drift.ring.pop_back();
            drift.ring.push_front(self.retimed_output(0, shift));
        }

        let unretimed = self.unretimed_echo();
        let estimate = interpolate(&drift.ring, drift.fraction);
        // How the estimate changes along time, from half a sample either side
        // of the delay: along the delay, the other way round.
        let slope = interpolate(&drift.ring, drift.fraction - 0.5)
            - interpolate(&drift.ring, drift.fraction + 0.5);
        let left = received - unretimed - estimate;
        self.meter(received, left);

        drift.recent.pop_back();
        drift.recent.push_front(received - unretimed);
        if drift.observe(left, estimate, slope, received) {
            self.search(drift);
        }
        left
    }

    /// One sample while the taps adapt: the delay goes on at the rate it had,
    /// and every retimed tap reads the reference through it.
    ///
    /// The loop learns nothing here, since what is left while the taps learn
    /// is theirs to explain. But the clocks have not stopped, and moving the
    /// delay on at the rate already measured keeps the echo standing still
    /// under the taps, so that they learn it as well as if it did not drift.
    /// Least mean squares cannot keep up with an echo moving under it: on a
    /// 100 ppm cable, the V.32 modem's first training leaves it 29 dB down
    /// where on a still one it reaches 39.
    ///
    /// The ring's shortcut does not hold here. It rests on the taps being one
    /// fixed filter, and while they learn they are a different one every
    /// sample; so each retimed tap has its own sample interpolated, and least
    /// mean squares runs on those exactly as it runs on the reference itself.
    /// It costs thirty-two times as much, for the few seconds of a retrain's
    /// training segment.
    fn learn_retimed(&mut self, drift: &mut Drift, transmitted: f64, received: f64) -> f64 {
        self.take_in(transmitted);
        drift.coast();
        let (first, phase) = place(drift.fraction);
        let row = &interpolator()[phase];
        let shift = drift.shift();

        drift.scratch.clear();
        let mut echo = 0.0;
        let mut energy = 0.0;
        for segment in std::iter::once(&self.near).chain(self.far.as_ref()) {
            let retimed = segment.first_retimed();
            for i in 0..retimed {
                let x = self.history[segment.offset + i];
                echo += segment.taps[i] * x;
                energy += x * x;
            }
            let readable = segment.first_readable(shift);
            for i in retimed..segment.taps.len() {
                let x = if i < readable {
                    0.0
                } else {
                    let start = (segment.offset as i64 + i as i64 + shift) as usize + first;
                    row.iter()
                        .enumerate()
                        .map(|(j, w)| w * self.history[start + j])
                        .sum()
                };
                drift.scratch.push(x);
                echo += segment.taps[i] * x;
                energy += x * x;
            }
        }
        let left = received - echo;
        self.meter(received, left);

        // The same step as the plain canceller's, over the same one division
        // by the energy under every tap, only of the reference each tap reads.
        let gain = self.step * left / (energy + FLOOR);
        let mut retimed_samples = drift.scratch.iter();
        for segment in std::iter::once(&mut self.near).chain(self.far.as_mut()) {
            let retimed = segment.first_retimed();
            for (i, tap) in segment.taps.iter_mut().enumerate() {
                let x = if i < retimed {
                    self.history[segment.offset + i]
                } else {
                    retimed_samples.next().copied().unwrap_or(0.0)
                };
                *tap += gain * x;
            }
        }

        // The taps are not the ones the ring was built from, and what is
        // left while they learn says nothing about the delay: once they stop,
        // it starts again from measuring.
        drift.stale = true;
        drift.held = None;
        drift.warming = WARM;
        drift.warm_sum = 0.0;
        left
    }

    /// Look for where the echo has jumped to, over whole samples either way,
    /// and go there if it is clearly better than where it was.
    ///
    /// The estimate is computed once, at the delay held, from a reach before
    /// the window to a reach past the present, and each shift is scored by
    /// how much of the window it leaves unexplained. Shifts that would have
    /// the estimate from the future have it from the taps that reach far
    /// enough back to supply it, which on a cable is all the ones that
    /// matter. About a million multiply-adds, once per fault.
    fn search(&self, drift: &mut Drift) {
        let (first, phase) = place(drift.fraction);
        let row = &interpolator()[phase];
        let shift = drift.shift();
        let window = SEARCH_WINDOW as i64;
        let reach = SEARCH_REACH as i64;

        // The estimate at `a` samples from now reads the taps' output at
        // `v = a - whole - first + 16 - j` for each interpolator tap `j`.
        let earliest = -(window - 1) - reach;
        let lowest = earliest - drift.whole - first as i64 - (HALF as i64 - 1);
        let highest = reach - drift.whole - first as i64 + HALF as i64;
        let output: Vec<f64> = (lowest..=highest)
            .map(|v| {
                std::iter::once(&self.near)
                    .chain(self.far.as_ref())
                    .map(|segment| {
                        let from = segment
                            .first_readable(shift)
                            .max((v - segment.offset as i64).max(0) as usize);
                        (from..segment.taps.len())
                            .map(|i| {
                                let back = (segment.offset + i) as i64 - v;
                                segment.taps[i] * self.history[back as usize]
                            })
                            .sum::<f64>()
                    })
                    .sum()
            })
            .collect();
        let estimate: Vec<f64> = (0..=(reach - earliest) as usize)
            .map(|k| {
                row.iter()
                    .enumerate()
                    .map(|(j, w)| w * output[k + 2 * HALF - 1 - j])
                    .sum()
            })
            .collect();

        let score = |s: i64| -> f64 {
            drift
                .recent
                .iter()
                .enumerate()
                .map(|(w, &heard)| {
                    let e = heard - estimate[(-(w as i64) - s - earliest) as usize];
                    e * e
                })
                .sum()
        };
        let stayed = score(0);
        let (best, least) = (-reach..=reach)
            .filter(|&s| s != 0)
            .map(|s| (s, score(s)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or((0, f64::INFINITY));

        // Believed when it leaves a fifth less than staying put, or when it
        // takes what is left back under the line that started the hold and
        // by at least half of what that line allows over normal.
        //
        // The second is for the smallest jump there is, a sample on a cable.
        // That costs about half the echo's power, and over 30 ms the far end
        // and the echo's own content are not steady enough for a fifth of
        // what is left to be certain: a dropped sample measured 0.85 of
        // staying put, with the sample back leaving 1.06 of normal against
        // staying put's 1.26. What no shift can do is take an eighth of
        // normal away by chance. A wrong one misplaces the whole echo, and
        // with no jump at all, a far end that got louder, every shift leaves
        // more than staying put does.
        let normal = drift.steady * SEARCH_WINDOW as f64;
        let believed = least < BETTER * stayed
            || (least < WORSE * normal && stayed - least > (WORSE - 1.0) / 2.0 * normal);
        if believed {
            drift.whole += best;
            drift.realigning = REALIGN_AFTER_JUMP;
            drift.jumps += 1;
            drift.stale = true;
        } else {
            // Nothing moved: whatever made what is left louder is on the line
            // and not in the canceller, and is the new normal.
            drift.steady = stayed / SEARCH_WINDOW as f64;
        }
        drift.held = None;
        drift.quick = drift.steady;
        drift.remember();
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
        // Where the echo had drifted to is learned too; following goes on,
        // from nothing.
        if let Some(drift) = self.drift.as_mut() {
            **drift = Drift::new();
        }
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

    #[test]
    fn the_interpolator_reads_between_samples_across_the_band() {
        // The ring holds the retimed taps' output, and a tap whose own lag
        // and the whole delay come to 16 reads it as the reference itself:
        // entry j is the tone j samples ago, and the point wanted is 16 and
        // the fraction back. Every fraction, both ends of the band.
        for f in [300.0, 1800.0, 3400.0] {
            let w = 2.0 * PI * f / 16_000.0;
            let mut worst: f64 = 0.0;
            for k in 0..=200 {
                let fraction = -0.5 + k as f64 / 200.0;
                for start in [0.0, 0.7, 1.9] {
                    let ring: VecDeque<f64> =
                        (0..RING).map(|j| (start - w * j as f64).cos()).collect();
                    let wanted = (start - w * (16.0 + fraction)).cos();
                    worst = worst.max((interpolate(&ring, fraction) - wanted).abs());
                }
            }
            // 64 dB measured at 3400 Hz, where rounding to the nearest of the
            // tabulated fractions is what is left.
            assert!(
                worst < 1.0e-3,
                "{f} Hz read {:.1} dB out between samples",
                20.0 * worst.log10()
            );
        }
    }

    #[test]
    fn a_whole_sample_of_delay_reads_the_sample_itself() {
        // What makes a delay that has not moved cost nothing: the taps read
        // exactly what they would without the interpolator in the way.
        let ring: VecDeque<f64> = (0..RING).map(|j| (j as f64 * 0.37).sin()).collect();
        assert_eq!(interpolate(&ring, 0.0), ring[HALF]);
    }

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
