//! What the V.32 rebuild has to achieve, asked of whole calls.
//!
//! Every test here is two [`Modem`]s in a call, each running the whole
//! start-up and carrying data, with nothing reached into but the modem's own
//! public face: `step`, `status`, `phase`, `retrains`, `send_bits`,
//! `take_bits` and the residual error. A bare receiver fed a clean signal can
//! be made to look well on any line; a call has to get through the start-up
//! first, train on what the start-up gives it, and then hold on while the line
//! does what lines do.
//!
//! The criteria are those of `docs/design/v32-rebuild/design.md` §9.2, and the
//! definitions are §9.1's:
//!
//! - **Es/N0** is white Gaussian noise at 16 kHz of variance
//!   `(fs/2)/baud · P · 10^(−SNR/10)`, where `P` is the far signal's power as
//!   it arrives. `P` is measured rather than assumed: one clean call at each
//!   rate, and the power of what the modems put out once connected.
//! - **Working SNR** is, for each rate, 3 dB above the worse of the ideal
//!   decoder's 1e-5 point and the point where the unchanged retrain rule fires.
//! - **Held** is the call reaching the rate and coding it was offered and
//!   staying `Connected` at it for the whole run, with no retrain at either
//!   end.
//! - **BER** and **blocks**: seeded pseudo-random bits both ways from a second
//!   after connecting, cut at each receiver into 100 ms blocks, and each block
//!   found in what the far end sent by searching ±2048 bits around where the
//!   last good block said it would be. A slip then costs the blocks it lands
//!   in rather than every bit after it. Every figure is taken at each end and
//!   the worse end is the one judged.
//!
//! # The line
//!
//! Each direction goes through one chain: a true single-sideband carrier shift
//! (lock_sweep's Hilbert `Shifter`), the far end's clock by resampling, a
//! delay, the noise, the jitter buffer's slips, and last the gain the network
//! or a softphone applies -- after the noise, so that a change of gain changes
//! the level and not the signal-to-noise ratio. A hybrid adds each modem's own
//! echo and the far hybrid's reflection, as `v32_call.rs`'s line with length
//! does. The cable is `soundcard_loop.rs`'s: both modems summed onto one wire
//! at 0.45 and heard back by both 700 samples later, with the drift and slips
//! a sound card adds.
//!
//! Where §9.2 names no noise for a line there is none. Those lines are about
//! slips, gains and cables; the ones that name a noise say so.
//!
//! Slip moments and every other random choice come from fixed seeds, so every
//! number here comes out the same every time.
//!
//! # Today and after
//!
//! Tests that fail on today's receiver carry
//! `#[ignore = "V.32 rebuild: enabled by package D"]`, and package D removes
//! the marks. What they were marked on is printed by the ignored
//! `before_and_after_table`, which runs every line below, and any one line can
//! be traced -- phases, errored blocks, slips -- with `one_line_traced`:
//!
//! ```text
//! cargo test -p datapump --release --test v32_acceptance before_and_after_table -- --ignored --nocapture
//! V32_LINE="cable 20 ppm" cargo test -p datapump --release --test v32_acceptance one_line_traced -- --ignored --nocapture
//! ```

use std::collections::VecDeque;
use std::f64::consts::{PI, TAU};
use std::fmt::Write as _;
use std::sync::OnceLock;

use datapump::v32::Coding;
use datapump::v32::startup::{Modem, Rates, Role, Status, rate_signal, rate_signal_v32};
use dsp::Resampler;

const FS: f64 = 16_000.0;

/// One block of the error count: 100 ms of whatever the receiver handed up.
const BLOCK: usize = (FS / 10.0) as usize;

/// How far either side of where a block is expected it is looked for, in bits
/// (§9.1).
const SEARCH: usize = 2048;

/// The hybrid of `v32_call.rs`: our own reflection 12 dB down, the far end 20.
const ECHO: f64 = 0.251;
const FAR: f64 = 0.1;
/// And the far hybrid's reflection of us, a whole round trip later.
const TALKER: f64 = 0.15;

/// `modem-loop`'s headroom for two modems summed onto one cable.
const HEADROOM: f64 = 0.45;
/// And one crossing of that cable, as `soundcard_loop.rs` measured it.
const CROSSING: usize = 700;

/// Twenty milliseconds, which is one packet of a jitter buffer.
const PACKET: usize = (0.020 * FS) as usize;

/// How long a start-up is given to connect.
const PATIENCE_S: f64 = 60.0;

// ---------------------------------------------------------------------------
// The rates
// ---------------------------------------------------------------------------

/// A rate and the coding it is carried with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Speed {
    At4800,
    /// 9600 without the trellis code (V.32 2.4.1.1).
    At9600U,
    At7200T,
    At9600T,
    At12000T,
    At14400T,
}

impl Speed {
    const ALL: [Speed; 6] = [
        Speed::At4800,
        Speed::At9600U,
        Speed::At7200T,
        Speed::At9600T,
        Speed::At12000T,
        Speed::At14400T,
    ];

    fn bits_per_second(self) -> u32 {
        match self {
            Self::At4800 => 4800,
            Self::At7200T => 7200,
            Self::At9600U | Self::At9600T => 9600,
            Self::At12000T => 12_000,
            Self::At14400T => 14_400,
        }
    }

    fn coding(self) -> Coding {
        match self {
            Self::At4800 | Self::At9600U => Coding::Uncoded,
            _ => Coding::Trellis,
        }
    }

    /// A rate signal offering this rate and nothing else.
    ///
    /// Between two V.32bis modems every rate above 4800 is trellis coded, so
    /// the uncoded 9600 is only reachable through V.32's own table with B8
    /// clear -- which is what a V.32 modem without 2.4.1.2 would send.
    fn offer(self) -> u16 {
        match self {
            Self::At9600U => rate_signal_v32(Rates::only(9600), false),
            _ => rate_signal(Rates::only(self.bits_per_second())),
        }
    }

    /// §9.1's working SNR, in dB of Es/N0.
    fn working_snr(self) -> f64 {
        match self {
            Self::At4800 => 15.5,
            Self::At9600U => 22.5,
            Self::At7200T => 18.0,
            Self::At9600T => 21.0,
            Self::At12000T => 24.0,
            Self::At14400T => 27.0,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::At4800 => "4800",
            Self::At9600U => "9600U",
            Self::At7200T => "7200T",
            Self::At9600T => "9600T",
            Self::At12000T => "12000T",
            Self::At14400T => "14400T",
        }
    }
}

/// The mean power a modem puts on the line once connected at `speed`.
///
/// Measured once per rate on a clean call rather than taken from the
/// transmitter's arithmetic: the Recommendation's own level statement puts
/// 12 000 and 14 400 a fraction of a decibel above the training states, and
/// whatever the transmitter makes of that, the noise has to be set against the
/// power that actually arrives.
fn transmitted_power(speed: Speed) -> f64 {
    static POWERS: [OnceLock<f64>; 6] = [const { OnceLock::new() }; 6];
    *POWERS[speed as usize].get_or_init(|| {
        let offer = speed.offer();
        let mut calling = Modem::new(Role::Calling, offer, FS);
        let mut answering = Modem::new(Role::Answering, offer, FS);
        let (mut from_calling, mut from_answering) = (0.0, 0.0);
        let mut up_at = None;
        let (mut sum, mut count) = (0.0, 0usize);
        for i in 0..(PATIENCE_S * FS) as usize {
            let (a, b) = (from_calling, from_answering);
            from_calling = calling.step(b);
            from_answering = answering.step(a);
            let _ = (calling.take_bits(), answering.take_bits());
            if up_at.is_none() && connected(&calling) && connected(&answering) {
                up_at = Some(i);
            }
            if let Some(up) = up_at {
                // Half a second in, when both ends are sending scrambled ones
                // at the rate, and then two seconds of it.
                if i > up + (0.5 * FS) as usize {
                    sum += from_calling * from_calling + from_answering * from_answering;
                    count += 2;
                }
                if i > up + (2.5 * FS) as usize {
                    break;
                }
            }
        }
        assert!(count > 0, "a clean call at {} never connected", speed.name());
        sum / count as f64
    })
}

fn connected(modem: &Modem) -> bool {
    matches!(modem.status(), Status::Connected(_))
}

// ---------------------------------------------------------------------------
// A seeded generator (lock_sweep's)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Rng {
    state: u64,
    spare: Option<f64>,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, spare: None }
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64*, which is short, fast and has no bad seeds but zero.
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn bit(&mut self) -> bool {
        self.next_u64() & 1 != 0
    }

    /// Box-Muller, with the second of each pair kept for next time.
    fn gaussian(&mut self) -> f64 {
        if let Some(g) = self.spare.take() {
            return g;
        }
        let u1 = self.unit().max(1e-18);
        let u2 = self.unit();
        let r = (-2.0 * u1.ln()).sqrt();
        self.spare = Some(r * (TAU * u2).sin());
        r * (TAU * u2).cos()
    }

    /// Put `items` in a random order.
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}

// ---------------------------------------------------------------------------
// The line
// ---------------------------------------------------------------------------

/// A single-sideband frequency shift: build the analytic signal with a Hilbert
/// transformer, rotate it, take the real part. Copied from `lock_sweep.rs`.
///
/// A plain multiply by a cosine would produce both sidebands and a plain
/// re-modulation would move the symbol rate along with the carrier. This moves
/// the carrier and nothing else, which is what a frequency-translating carrier
/// system on a real trunk does.
#[derive(Debug)]
struct Shifter {
    /// `h[d]`, paired with `x[n-d]`.
    taps: Vec<f64>,
    hist: Vec<f64>,
    pos: usize,
    centre: usize,
    phase: f64,
    step: f64,
}

impl Shifter {
    fn new(hz: f64, fs: f64) -> Self {
        const N: usize = 255;
        let m = (N - 1) / 2;
        let mut taps = vec![0.0; N];
        for (d, tap) in taps.iter_mut().enumerate() {
            let k = d as isize - m as isize;
            if k % 2 == 0 {
                continue;
            }
            // Hamming, over the whole length.
            let w = 0.54 - 0.46 * (TAU * d as f64 / (N - 1) as f64).cos();
            *tap = 2.0 / (PI * k as f64) * w;
        }
        Self {
            taps,
            hist: vec![0.0; N],
            pos: 0,
            centre: m,
            phase: 0.0,
            step: hz / fs,
        }
    }

    fn process(&mut self, x: f64) -> f64 {
        let n = self.hist.len();
        self.pos = (self.pos + 1) % n;
        self.hist[self.pos] = x;
        let mut q = 0.0;
        for (d, &tap) in self.taps.iter().enumerate() {
            if tap != 0.0 {
                q += tap * self.hist[(self.pos + n - d) % n];
            }
        }
        let i = self.hist[(self.pos + n - self.centre) % n];
        let turn = self.phase * TAU;
        self.phase += self.step;
        self.phase -= self.phase.floor();
        i * turn.cos() - q * turn.sin()
    }
}

/// What a jitter buffer makes up in a hole it has to fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// The last packet over again, faded across both joins as packet loss
    /// concealment does: V.34's `slip` (`v34/receiver.rs`), streamed.
    Repeat,
    /// Noise at the level of the last packet, as a buffer with comfort noise
    /// plays.
    Comfort,
    /// Nothing at all, as a buffer with no concealer, or a sound card that has
    /// run dry, plays.
    Silence,
}

/// One slip of the line's clock against the receiver's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slip {
    /// This many samples lost.
    Drop(usize),
    /// This many samples made up and played ahead of what was due, which is
    /// then that much later.
    Insert(usize, Fill),
}

/// A softphone's gain control: anything louder than the ceiling is turned down
/// to it at once, and the gain comes back up with a one-second time constant
/// -- `v90::network`'s model of what a live call through a softphone did.
#[derive(Debug)]
struct Limiter {
    ceiling: f64,
    gain: f64,
    /// When the click that sets it off arrives.
    click_at: Option<usize>,
    /// Samples it has turned down, the click's included.
    clamped: usize,
}

/// How long the limiter takes to let go.
const RELEASE_S: f64 = 1.0;

/// The limiter's ceiling, against the root-mean-square of the far signal.
///
/// Far above the peaks of any V.32 constellation after shaping, even 6 dB up,
/// so data does not set it off. A click does, in the start-up, and what the
/// receiver then has to follow is the second or two of the gain coming back.
const CEILING: f64 = 10.0;

/// How far the click turns the gain down.
const LIMITED_DB: f64 = 10.0;

impl Limiter {
    fn process(&mut self, x: f64, now: usize) -> f64 {
        let mut x = x;
        if self.click_at.is_some_and(|at| at <= now) {
            self.click_at = None;
            x += self.ceiling * 10f64.powf(LIMITED_DB / 20.0);
        }
        if (x * self.gain).abs() > self.ceiling {
            self.gain = self.ceiling / x.abs();
            self.clamped += 1;
        }
        let y = x * self.gain;
        self.gain += (1.0 - self.gain) / (RELEASE_S * FS);
        y
    }
}

/// What one direction of a two-wire line does to the signal crossing it.
#[derive(Debug, Clone)]
struct Direction {
    attenuation: f64,
    carrier_hz: f64,
    /// The far end's clock against ours, in parts per million.
    ppm: f64,
    /// In samples: what goes in at one step comes out `delay` steps later.
    delay: usize,
    snr_db: Option<f64>,
    /// Seconds from the start of the call.
    slips_from_start: Vec<(f64, Slip)>,
    /// Seconds from the start of data.
    slips_in_data: Vec<(f64, Slip)>,
    /// Seconds from the start of data, seconds taken, and the gain it ends at.
    gains_in_data: Vec<(f64, f64, f64)>,
    /// A softphone's limiter, set off as the far end's first conditioning
    /// signal arrives.
    limiter: bool,
    /// The far end replaced by loud noise: when, from the start of data, and
    /// for how long.
    loss_in_data: Option<(f64, f64)>,
}

impl Default for Direction {
    fn default() -> Self {
        Self {
            attenuation: 1.0,
            carrier_hz: 0.0,
            ppm: 0.0,
            delay: 1,
            snr_db: None,
            slips_from_start: Vec::new(),
            slips_in_data: Vec::new(),
            gains_in_data: Vec::new(),
            limiter: false,
            loss_in_data: None,
        }
    }
}

/// One direction of the line while a call is on it: the chain in the module
/// comment, a sample in and a sample out.
#[derive(Debug)]
struct Path {
    attenuation: f64,
    shifter: Option<Shifter>,
    clock: Option<Resampler>,
    resampled: Vec<f64>,
    /// The line's delay, and the jitter buffer's contents.
    fifo: VecDeque<f64>,
    noise: f64,
    rng: Rng,
    /// Slips still to come, soonest first, by the sample they happen at.
    slips: VecDeque<(usize, Slip)>,
    /// When each slip happened.
    slipped: Vec<usize>,
    /// What the buffer has made up and not yet played.
    made_up: VecDeque<f64>,
    /// The last packet played, which is what a repeat or comfort noise is made
    /// from.
    recent: VecDeque<f64>,
    /// Gain changes still to come: when, over how many samples, and to what.
    gains: VecDeque<(usize, usize, f64)>,
    gain_db: f64,
    gain_step: f64,
    gain_left: usize,
    limiter: Option<Limiter>,
    /// A stretch where the far end is replaced by noise of this deviation.
    loss: Option<(usize, usize, f64)>,
    now: usize,
    /// Samples the line had nothing to give for, which only a mistake in this
    /// file could cause.
    underruns: usize,
}

impl Path {
    /// `power` is the far modem's, before the line attenuates it.
    fn new(direction: &Direction, power: f64, seed: u64) -> Self {
        let shifter = (direction.carrier_hz != 0.0).then(|| Shifter::new(direction.carrier_hz, FS));
        let clock =
            (direction.ppm != 0.0).then(|| Resampler::new(FS * (1.0 + direction.ppm / 1e6), FS));
        // The resampler holds its first sixteen outputs back while its kernel
        // fills, a far clock that runs fast drains the buffer by `ppm` of a
        // sample a sample, and samples dropped come out of it too. The buffer
        // starts with enough for all of that, so it never runs dry; the delay
        // is then the one asked for plus whatever the drift has still to eat.
        let lead = if clock.is_some() { 16 } else { 0 };
        let drift = (direction.ppm.max(0.0) * 1e-6 * FS * 200.0).ceil() as usize;
        let dropped: usize = direction
            .slips_from_start
            .iter()
            .chain(&direction.slips_in_data)
            .map(|&(_, slip)| if let Slip::Drop(n) = slip { n } else { 0 })
            .sum();
        let fill = direction.delay.max(1) - 1 + lead + drift + dropped;
        let received = direction.attenuation * direction.attenuation * power;
        let noise = direction
            .snr_db
            .map_or(0.0, |snr| (FS / 2.0 / 2400.0 * received * 10f64.powf(-snr / 10.0)).sqrt());
        let mut path = Self {
            attenuation: direction.attenuation,
            shifter,
            clock,
            resampled: Vec::new(),
            fifo: VecDeque::from(vec![0.0; fill]),
            noise,
            rng: Rng::new(seed),
            slips: VecDeque::new(),
            slipped: Vec::new(),
            made_up: VecDeque::new(),
            recent: VecDeque::from(vec![0.0; PACKET]),
            gains: VecDeque::new(),
            gain_db: 0.0,
            gain_step: 0.0,
            gain_left: 0,
            limiter: direction.limiter.then(|| Limiter {
                ceiling: CEILING * received.sqrt(),
                gain: 1.0,
                click_at: None,
                clamped: 0,
            }),
            loss: None,
            now: 0,
            underruns: 0,
        };
        path.schedule_slips(0, &direction.slips_from_start);
        path
    }

    fn schedule_slips(&mut self, from: usize, slips: &[(f64, Slip)]) {
        self.slips.extend(slips.iter().map(|&(at, slip)| (from + (at * FS) as usize, slip)));
        self.slips.make_contiguous().sort_by_key(|&(at, _)| at);
    }

    /// Everything that is timed from the moment data starts.
    fn data_starts(&mut self, direction: &Direction, power: f64) {
        let from = self.now;
        self.schedule_slips(from, &direction.slips_in_data);
        for &(at, over, to_db) in &direction.gains_in_data {
            self.gains.push_back((from + (at * FS) as usize, (over * FS) as usize, to_db));
        }
        if let Some((at, seconds)) = direction.loss_in_data {
            // Loud: four times the power of the far end it replaces.
            let sigma = 2.0 * direction.attenuation * power.sqrt();
            let at = from + (at * FS) as usize;
            self.loss = Some((at, at + (seconds * FS) as usize, sigma));
        }
    }

    fn carry(&mut self, x: f64) -> f64 {
        let mut x = x * self.attenuation;
        if let Some(shifter) = self.shifter.as_mut() {
            x = shifter.process(x);
        }
        if let Some(clock) = self.clock.as_mut() {
            self.resampled.clear();
            clock.process(x, &mut self.resampled);
            self.fifo.extend(self.resampled.iter().copied());
        } else {
            self.fifo.push_back(x);
        }

        while let Some(&(at, slip)) = self.slips.front() {
            if at > self.now {
                break;
            }
            self.slips.pop_front();
            self.slipped.push(self.now);
            match slip {
                Slip::Drop(n) => {
                    for _ in 0..n {
                        if self.fifo.pop_front().is_none() {
                            self.underruns += 1;
                        }
                    }
                }
                Slip::Insert(n, fill) => self.make_up(n, fill),
            }
        }

        let mut y = if let Some(made) = self.made_up.pop_front() {
            made
        } else if let Some(arrived) = self.fifo.pop_front() {
            match self.loss {
                Some((from, until, sigma)) if (from..until).contains(&self.now) => sigma * self.rng.gaussian(),
                _ => arrived + self.noise * self.rng.gaussian(),
            }
        } else {
            self.underruns += 1;
            0.0
        };
        self.recent.pop_front();
        self.recent.push_back(y);

        if self.gain_left == 0
            && let Some(&(at, over, to_db)) = self.gains.front()
            && at <= self.now
        {
            self.gains.pop_front();
            if over == 0 {
                self.gain_db = to_db;
            } else {
                self.gain_step = (to_db - self.gain_db) / over as f64;
                self.gain_left = over;
            }
        }
        if self.gain_left > 0 {
            self.gain_db += self.gain_step;
            self.gain_left -= 1;
        }
        if self.gain_db != 0.0 {
            y *= 10f64.powf(self.gain_db / 20.0);
        }
        if let Some(limiter) = self.limiter.as_mut() {
            y = limiter.process(y, self.now);
        }
        self.now += 1;
        y
    }

    /// What a jitter buffer plays into a hole of `n` samples.
    fn make_up(&mut self, n: usize, fill: Fill) {
        let last: Vec<f64> = self.recent.iter().rev().take(n).rev().copied().collect();
        match fill {
            Fill::Repeat if n == 1 => self.made_up.push_back(last[0]),
            Fill::Repeat => {
                // Faded across both joins, forty samples each side.
                let fade = 40.min(n / 2);
                for (i, &x) in last.iter().enumerate() {
                    let edge = i.min(n - 1 - i);
                    let scale = if edge < fade { edge as f64 / fade as f64 } else { 1.0 };
                    self.made_up.push_back(x * scale);
                }
            }
            Fill::Comfort => {
                let rms = (last.iter().map(|x| x * x).sum::<f64>() / n as f64).sqrt();
                for _ in 0..n {
                    let v = rms * self.rng.gaussian();
                    self.made_up.push_back(v);
                }
            }
            Fill::Silence => self.made_up.extend(std::iter::repeat_n(0.0, n)),
        }
    }
}

// One is made per call, so the size of the larger variant costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
enum Line {
    /// Two directions, each its own path. With `hybrid`, each modem also hears
    /// its own echo at once and the far hybrid's reflection of it that many
    /// samples later, as `v32_call.rs`'s line with length does.
    TwoWire { to_calling: Direction, to_answering: Direction, hybrid: Option<usize> },
    /// A sound card looped back: both modems summed onto one wire and heard by
    /// both. The card reads `ppm` faster than it writes, so the crossing
    /// lengthens through the call, and the slips are the card's own.
    Cable { ppm: f64, slips_in_data: Vec<(f64, Slip)> },
}

impl Line {
    /// How long what reaches `role` has been on its way, in samples.
    fn delay_to(&self, role: Role) -> usize {
        match (self, role) {
            (Self::TwoWire { to_calling, .. }, Role::Calling) => to_calling.delay,
            (Self::TwoWire { to_answering, .. }, Role::Answering) => to_answering.delay,
            (Self::Cable { .. }, _) => CROSSING,
        }
    }

    /// When the far end is lost, in seconds from the start of data.
    fn loss(&self) -> Option<f64> {
        match self {
            Self::TwoWire { to_calling, to_answering, .. } => {
                to_calling.loss_in_data.or(to_answering.loss_in_data).map(|(at, _)| at)
            }
            Self::Cable { .. } => None,
        }
    }
}

/// The line while a call is on it.
#[derive(Debug)]
enum Wiring {
    TwoWire {
        to_calling: Box<Path>,
        to_answering: Box<Path>,
        /// Each end's own output, kept for the far reflection.
        hybrid: Option<(VecDeque<f64>, VecDeque<f64>)>,
    },
    Cable(Box<Path>),
}

impl Wiring {
    fn new(line: &Line, power: f64, seed: u64) -> Self {
        match line {
            Line::TwoWire { to_calling, to_answering, hybrid } => Self::TwoWire {
                to_calling: Box::new(Path::new(to_calling, power, seed ^ 0x5151)),
                to_answering: Box::new(Path::new(to_answering, power, seed ^ 0xA3A3)),
                hybrid: hybrid.map(|back| {
                    let empty = || VecDeque::from(vec![0.0; back]);
                    (empty(), empty())
                }),
            },
            Line::Cable { ppm, slips_in_data } => {
                let wire = Direction {
                    // The card reads faster than it writes: see `Line::Cable`.
                    ppm: -ppm,
                    delay: CROSSING,
                    slips_in_data: slips_in_data.clone(),
                    ..Direction::default()
                };
                Self::Cable(Box::new(Path::new(&wire, power, seed)))
            }
        }
    }

    /// What each end hears, given what each said last.
    fn step(&mut self, from_calling: f64, from_answering: f64) -> (f64, f64) {
        match self {
            Self::TwoWire { to_calling, to_answering, hybrid } => {
                let mut heard_by_calling = to_calling.carry(from_answering);
                let mut heard_by_answering = to_answering.carry(from_calling);
                if let Some((calling_sent, answering_sent)) = hybrid.as_mut() {
                    calling_sent.push_back(from_calling);
                    answering_sent.push_back(from_answering);
                    let calling_back = calling_sent.pop_front().unwrap_or(0.0);
                    let answering_back = answering_sent.pop_front().unwrap_or(0.0);
                    heard_by_calling += ECHO * from_calling + TALKER * calling_back;
                    heard_by_answering += ECHO * from_answering + TALKER * answering_back;
                }
                (heard_by_calling, heard_by_answering)
            }
            Self::Cable(wire) => {
                let heard = wire.carry((from_calling + from_answering) * HEADROOM);
                (heard, heard)
            }
        }
    }

    fn data_starts(&mut self, line: &Line, power: f64) {
        match (self, line) {
            (
                Self::TwoWire { to_calling, to_answering, .. },
                Line::TwoWire { to_calling: c, to_answering: a, .. },
            ) => {
                to_calling.data_starts(c, power);
                to_answering.data_starts(a, power);
            }
            (Self::Cable(wire), Line::Cable { slips_in_data, .. }) => {
                wire.schedule_slips(wire.now, slips_in_data);
            }
            _ => unreachable!("the wiring is made from the line"),
        }
    }

    /// The path that ends at `role`.
    fn heard_by(&self, role: Role) -> &Path {
        match (self, role) {
            (Self::TwoWire { to_calling, .. }, Role::Calling) => to_calling,
            (Self::TwoWire { to_answering, .. }, Role::Answering) => to_answering,
            (Self::Cable(wire), _) => wire,
        }
    }

    fn heard_by_mut(&mut self, role: Role) -> &mut Path {
        match (self, role) {
            (Self::TwoWire { to_calling, .. }, Role::Calling) => to_calling,
            (Self::TwoWire { to_answering, .. }, Role::Answering) => to_answering,
            (Self::Cable(wire), _) => wire,
        }
    }

    fn underruns(&self) -> usize {
        match self {
            Self::TwoWire { to_calling, to_answering, .. } => to_calling.underruns + to_answering.underruns,
            Self::Cable(wire) => wire.underruns,
        }
    }
}

// ---------------------------------------------------------------------------
// Counting errors
// ---------------------------------------------------------------------------

/// Bits packed 64 to a word, first bit lowest, for comparing at any offset.
#[derive(Debug, Default, Clone)]
struct Packed {
    words: Vec<u64>,
    len: usize,
}

impl Packed {
    fn push(&mut self, bit: bool) {
        if self.len.is_multiple_of(64) {
            self.words.push(0);
        }
        if bit {
            *self.words.last_mut().expect("just pushed") |= 1 << (self.len % 64);
        }
        self.len += 1;
    }

    fn from_bits(bits: &[bool]) -> Self {
        let mut packed = Self::default();
        for &bit in bits {
            packed.push(bit);
        }
        packed
    }

    /// The 64 bits from `at` on, with zeros past the end.
    fn word(&self, at: usize) -> u64 {
        let (w, b) = (at / 64, at % 64);
        let low = self.words.get(w).copied().unwrap_or(0);
        if b == 0 {
            return low;
        }
        let high = self.words.get(w + 1).copied().unwrap_or(0);
        (low >> b) | (high << (64 - b))
    }

    /// Bits of `block` that differ from this stream laid over it at `at`,
    /// giving up once past `enough`.
    fn distance(&self, block: &Packed, at: usize, enough: usize) -> usize {
        let mut wrong = 0;
        for (j, &w) in block.words.iter().enumerate() {
            let used = (block.len - 64 * j).min(64);
            let mask = if used == 64 { u64::MAX } else { (1u64 << used) - 1 };
            wrong += ((w ^ self.word(at + 64 * j)) & mask).count_ones() as usize;
            if wrong > enough {
                break;
            }
        }
        wrong
    }
}

/// Finds each block of what one end received in what the other end sent.
#[derive(Debug, Default)]
struct Aligner {
    /// Where the next block should start in what was sent, once one block has
    /// been found.
    expected: Option<usize>,
    /// Blocks in a row that were not found near where they should have been.
    lost: usize,
}

impl Aligner {
    /// The bits wrong in one block, where it best fits within ±2048 bits of
    /// where it was expected.
    fn place(&mut self, sent: &Packed, block: &Packed) -> usize {
        let n = block.len;
        if n < 64 || sent.len < n {
            self.expected = self.expected.map(|e| e + n);
            self.lost += 1;
            return n;
        }
        let last = sent.len - n;
        // Found, rather than merely compared: an eighth of the bits wrong is
        // a block with errors in it, and half of them is one lined up against
        // the wrong part of the stream.
        let found = n / 8;
        let mut best: Option<(usize, usize)> = None;
        if let Some(e) = self.expected {
            let e = e.min(last);
            let here = sent.distance(block, e, n);
            best = Some((here, e));
            if here > 0 {
                for at in e.saturating_sub(SEARCH)..=(e + SEARCH).min(last) {
                    let d = sent.distance(block, at, best.map_or(n, |b| b.0));
                    if d < best.map_or(usize::MAX, |b| b.0) {
                        best = Some((d, at));
                    }
                }
            }
        }
        let counted = best.map_or(n, |b| b.0);
        // Never found yet, or lost for a second: look through everything sent.
        // A retrain then costs its own length rather than the rest of the
        // call, while a slip is always inside the ±2048 above.
        if counted > found && (self.expected.is_none() || self.lost >= 10) {
            let first = block.words[0];
            let mut anywhere: Option<(usize, usize)> = None;
            for at in 0..=last {
                if (first ^ sent.word(at)).count_ones() <= 8 {
                    let d = sent.distance(block, at, n);
                    if d < anywhere.map_or(usize::MAX, |f| f.0) {
                        anywhere = Some((d, at));
                    }
                }
            }
            if let Some((d, at)) = anywhere.filter(|f| f.0 <= found) {
                if self.expected.is_none() {
                    best = Some((d, at));
                } else {
                    // Found again, for the blocks after this one.
                    self.expected = Some(at + n);
                    self.lost = 0;
                    return counted;
                }
            }
        }
        match best {
            Some((d, at)) if d <= found => {
                self.expected = Some(at + n);
                self.lost = 0;
                d
            }
            _ => {
                self.expected = self.expected.map(|e| e + n);
                self.lost += 1;
                best.map_or(n, |b| b.0)
            }
        }
    }
}

/// How one end's reception went.
#[derive(Debug, Clone, Default)]
struct Reception {
    blocks: usize,
    clean: usize,
    bits: usize,
    errors: usize,
    /// When each errored block ended, in samples from the start of the call.
    errored: Vec<usize>,
    /// Median of residual error over point spacing, while connected in data.
    reception: f64,
    /// When each slip reached this end, while it was being measured.
    slips: Vec<usize>,
}

/// How long after a slip the errored blocks are counted as its cost.
const SLIP_COST_WINDOW: usize = (2.0 * FS) as usize;

impl Reception {
    fn errored_share(&self) -> f64 {
        if self.blocks == 0 { 1.0 } else { 1.0 - self.clean as f64 / self.blocks as f64 }
    }

    fn ber(&self) -> f64 {
        if self.bits == 0 { 0.5 } else { self.errors as f64 / self.bits as f64 }
    }

    /// Errored blocks ending in the two seconds after each slip.
    fn slip_costs(&self) -> Vec<usize> {
        self.slips
            .iter()
            .map(|&at| self.errored.iter().filter(|&&end| end > at && end <= at + SLIP_COST_WINDOW).count())
            .collect()
    }

    /// The most errored time any one slip cost, in seconds.
    fn worst_slip(&self) -> f64 {
        self.slip_costs().into_iter().max().unwrap_or(0) as f64 * 0.1
    }
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

// ---------------------------------------------------------------------------
// A call
// ---------------------------------------------------------------------------

/// A call to be made, and what is expected of it.
#[derive(Debug, Clone)]
struct Call {
    offer: u16,
    /// The rate and coding it has to hold, which also sets the noise.
    speed: Speed,
    line: Line,
    /// Seconds of data measured at each end.
    data_s: f64,
    seed: u64,
    /// Print the start-up's phases and every errored block.
    trace: bool,
}

/// What happened.
#[derive(Debug, Clone)]
struct Outcome {
    /// The rate both ends first came up at, and its coding.
    first: Option<(u32, Coding)>,
    /// Whether the rate and coding asked for were reached and never left.
    held: bool,
    last: [Status; 2],
    retrains: [u32; 2],
    /// Seconds from the far end being lost to the first retrain after it.
    retrain_after_loss: Option<f64>,
    /// At the calling end and at the answering end.
    ends: [Reception; 2],
    underruns: usize,
}

impl Call {
    fn run(&self) -> Outcome {
        let power = transmitted_power(self.speed);
        let mut calling = Modem::new(Role::Calling, self.offer, FS);
        let mut answering = Modem::new(Role::Answering, self.offer, FS);
        let mut wiring = Wiring::new(&self.line, power, self.seed);
        let (mut from_calling, mut from_answering) = (0.0, 0.0);

        let want = (self.speed.bits_per_second(), self.speed.coding());
        let mut first = None;
        let mut up_at = None;
        let mut left = false;
        let mut data_at = None;
        let mut loss_at = None;
        let mut retrain_after_loss = None;
        let mut conditioning = [false; 2];

        let mut rng = [Rng::new(self.seed ^ 0xC0FFEE), Rng::new(self.seed ^ 0xBEEF)];
        let mut sent = [Packed::default(), Packed::default()];
        let mut block = [Vec::new(), Vec::new()];
        let mut received: [Vec<(Packed, usize)>; 2] = [Vec::new(), Vec::new()];
        let mut listening_from = [usize::MAX; 2];
        let mut reception: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
        let mut end = (PATIENCE_S * FS) as usize;
        let blocks = (self.data_s * 10.0) as usize;

        let mut phases = ("", "");
        let mut i = 0;
        while i < end {
            if self.trace && (calling.phase(), answering.phase()) != phases {
                phases = (calling.phase(), answering.phase());
                println!(
                    "{:8.3} s  calling {:>12} {:?}  answering {:>12} {:?}  reception {:.3} / {:.3}",
                    i as f64 / FS,
                    phases.0,
                    calling.status(),
                    phases.1,
                    answering.status(),
                    calling.residual_error() / calling.point_spacing(),
                    answering.residual_error() / answering.point_spacing()
                );
            }
            let (to_calling, to_answering) = wiring.step(from_calling, from_answering);
            from_calling = calling.step(to_calling);
            from_answering = answering.step(to_answering);

            // A softphone's limiter set off just as the far end's first
            // conditioning signal reaches it: the receiver trains while the
            // gain is still coming back, and goes into data with it rising.
            if i.is_multiple_of(16) {
                for (k, (far, listener, phase)) in [
                    (&answering, Role::Calling, "S"),
                    (&calling, Role::Answering, "S pre-roll"),
                ]
                .into_iter()
                .enumerate()
                {
                    if !conditioning[k] && far.phase() == phase {
                        conditioning[k] = true;
                        let arrives = i + self.line.delay_to(listener);
                        if let Some(limiter) = wiring.heard_by_mut(listener).limiter.as_mut() {
                            limiter.click_at = Some(arrives);
                        }
                    }
                }
            }

            let both = (calling.status(), answering.status());
            if up_at.is_none()
                && let (Status::Connected(rate), Status::Connected(_)) = both
            {
                up_at = Some(i);
                first = Some((rate, calling.coding()));
                data_at = Some(i + FS as usize);
            }
            if up_at.is_some() {
                let at_rate = |s: Status| s == Status::Connected(want.0);
                if !at_rate(both.0) || !at_rate(both.1) {
                    left = true;
                }
                if let (Some(loss), None) = (loss_at, retrain_after_loss)
                    && i >= loss
                    && (both.0 == Status::Retraining || both.1 == Status::Retraining)
                {
                    retrain_after_loss = Some((i - loss) as f64 / FS);
                }
            }

            if data_at == Some(i) {
                wiring.data_starts(&self.line, power);
                for (k, role) in [Role::Calling, Role::Answering].into_iter().enumerate() {
                    // What reaches this end has been on its way for the line's
                    // delay; a quarter of a second more and the far end's data
                    // is certainly arriving.
                    listening_from[k] = i + self.line.delay_to(role) + (0.25 * FS) as usize;
                }
                end = listening_from.iter().max().copied().unwrap_or(i) + blocks * BLOCK;
                loss_at = self.line.loss().map(|at| i + (at * FS) as usize);
            }

            if data_at.is_some_and(|data| i >= data) {
                for (k, modem) in [&mut calling, &mut answering].into_iter().enumerate() {
                    if modem.pending_bits() < 128 {
                        let bits: Vec<bool> = (0..256).map(|_| rng[k].bit()).collect();
                        for &bit in &bits {
                            sent[k].push(bit);
                        }
                        modem.send_bits(&bits);
                    }
                }
            }

            for (k, modem) in [&mut calling, &mut answering].into_iter().enumerate() {
                let bits = modem.take_bits();
                let from = listening_from[k];
                if i < from {
                    continue;
                }
                block[k].extend(bits);
                if (i - from) % BLOCK == BLOCK - 1 && received[k].len() < blocks {
                    received[k].push((Packed::from_bits(&block[k]), i));
                    block[k].clear();
                }
                if i.is_multiple_of(16) && connected(modem) {
                    reception[k].push(modem.residual_error() / modem.point_spacing());
                }
            }
            i += 1;
        }

        let held = first == Some(want) && !left && calling.retrains() == 0 && answering.retrains() == 0;
        let mut ends = [Reception::default(), Reception::default()];
        for (k, role) in [Role::Calling, Role::Answering].into_iter().enumerate() {
            // What the calling end received was sent by the answering end.
            let far = &sent[1 - k];
            let mut aligner = Aligner::default();
            let r = &mut ends[k];
            for (bits, ended) in &received[k] {
                let wrong = aligner.place(far, bits);
                r.blocks += 1;
                r.bits += bits.len;
                r.errors += wrong;
                if wrong == 0 && bits.len > 0 {
                    r.clean += 1;
                } else {
                    r.errored.push(*ended);
                    if self.trace {
                        println!(
                            "  {role:?} end: block ending {:.2} s, {wrong} of {} bits wrong",
                            *ended as f64 / FS,
                            bits.len
                        );
                    }
                }
            }
            r.reception = median(&mut reception[k]);
            let path = wiring.heard_by(role);
            r.slips = path.slipped.iter().copied().filter(|&at| at >= listening_from[k] && at < end).collect();
            if self.trace {
                let seconds = |at: &usize| (*at as f64 / FS * 100.0).round() / 100.0;
                println!(
                    "{role:?} end: slips at {:?} s, costing {:?} blocks",
                    r.slips.iter().map(seconds).collect::<Vec<_>>(),
                    r.slip_costs()
                );
                if let Some(limiter) = path.limiter.as_ref() {
                    println!("{role:?} end: the limiter turned {} samples down", limiter.clamped);
                }
            }
        }
        Outcome {
            first,
            held,
            last: [calling.status(), answering.status()],
            retrains: [calling.retrains(), answering.retrains()],
            retrain_after_loss,
            ends,
            underruns: wiring.underruns(),
        }
    }
}

// ---------------------------------------------------------------------------
// What each test asks
// ---------------------------------------------------------------------------

/// A pass criterion from §9.2.
#[derive(Debug, Clone, Copy)]
enum Pass {
    /// Held, and at most one bit in 100 000 wrong at either end.
    Ber,
    /// Held, and at least this share of blocks clean at each end.
    Clean(f64),
    /// Held, at least this share of blocks clean, and no one slip costing more
    /// than 200 ms of errored blocks.
    Slips(f64),
    /// Held.
    Held,
    /// Exactly one retrain at each end, begun within 1.6 s of the far end
    /// being lost, and back at this rate.
    OneRetrain(u32),
}

/// One line of §9.2: a test's call at one rate or one setting.
#[derive(Debug, Clone)]
struct Row {
    test: &'static str,
    label: String,
    call: Call,
    pass: Pass,
}

impl Row {
    fn verdict(&self, o: &Outcome) -> Result<(), String> {
        let held = || {
            if o.held {
                Ok(())
            } else {
                Err(format!(
                    "not held: came up at {:?}, ended {:?} / {:?} after {} / {} retrains",
                    o.first, o.last[0], o.last[1], o.retrains[0], o.retrains[1]
                ))
            }
        };
        let worst = |f: fn(&Reception) -> f64| f(&o.ends[0]).max(f(&o.ends[1]));
        match self.pass {
            Pass::Held => held(),
            Pass::Ber => {
                held()?;
                let ber = worst(Reception::ber);
                if ber <= 1e-5 { Ok(()) } else { Err(format!("BER {ber:.1e}")) }
            }
            Pass::Clean(share) => {
                held()?;
                let errored = worst(Reception::errored_share);
                if 1.0 - errored >= share {
                    Ok(())
                } else {
                    Err(format!("{:.1} % of blocks errored", 100.0 * errored))
                }
            }
            Pass::Slips(share) => {
                held()?;
                let errored = worst(Reception::errored_share);
                let slip = worst(Reception::worst_slip);
                if 1.0 - errored < share {
                    Err(format!("{:.1} % of blocks errored", 100.0 * errored))
                } else if slip > 0.2 + 1e-9 {
                    Err(format!("a slip cost {:.0} ms of errored blocks", 1000.0 * slip))
                } else {
                    Ok(())
                }
            }
            Pass::OneRetrain(rate) => {
                if o.retrains != [1, 1] {
                    return Err(format!("{} / {} retrains", o.retrains[0], o.retrains[1]));
                }
                match o.retrain_after_loss {
                    Some(after) if after <= 1.6 => {}
                    Some(after) => return Err(format!("the retrain began {after:.2} s after the loss")),
                    None => return Err("no retrain after the loss".into()),
                }
                if o.last != [Status::Connected(rate); 2] {
                    return Err(format!("came back at {:?} / {:?}", o.last[0], o.last[1]));
                }
                Ok(())
            }
        }
    }

    /// The row as the before-and-after table prints it.
    fn line(&self, o: &Outcome) -> String {
        let rate = |s: Status| match s {
            Status::Connected(r) => r.to_string(),
            other => format!("{other:?}"),
        };
        let first = o.first.map_or("never".to_string(), |(r, c)| {
            format!("{r}{}", if c == Coding::Trellis { "T" } else { "" })
        });
        let mut notes = String::new();
        if let Some(after) = o.retrain_after_loss {
            let _ = write!(notes, "retrain {after:.2} s after the loss; ");
        }
        let slip = o.ends[0].worst_slip().max(o.ends[1].worst_slip());
        if !o.ends[0].slips.is_empty() || !o.ends[1].slips.is_empty() {
            let _ = write!(notes, "worst slip {:.0} ms; ", 1000.0 * slip);
        }
        let verdict = match self.verdict(o) {
            Ok(()) => "pass".to_string(),
            Err(why) => format!("FAIL: {why}"),
        };
        format!(
            "| {} | {} | {} | {} / {} | {} / {} | {:.1} / {:.1} % | {:.1e} | {:.3} / {:.3} | {notes}{verdict} |",
            self.test,
            self.label,
            first,
            rate(o.last[0]),
            rate(o.last[1]),
            o.retrains[0],
            o.retrains[1],
            100.0 * o.ends[0].errored_share(),
            100.0 * o.ends[1].errored_share(),
            o.ends[0].ber().max(o.ends[1].ber()),
            o.ends[0].reception,
            o.ends[1].reception,
        )
    }
}

/// Run calls side by side, one thread each: they are independent, and each is
/// seconds of arithmetic.
fn run_all(rows: &[Row]) -> Vec<Outcome> {
    std::thread::scope(|scope| {
        let running: Vec<_> = rows
            .iter()
            .map(|row| {
                std::thread::Builder::new()
                    .stack_size(16 << 20)
                    .spawn_scoped(scope, || row.call.run())
                    .expect("a thread for the call")
            })
            .collect();
        running.into_iter().map(|h| h.join().expect("a call panicked")).collect()
    })
}

/// Run one test's rows and fail with every row that did not pass.
fn check(test: &str) {
    let rows: Vec<Row> = every_row().into_iter().filter(|r| r.test == test).collect();
    assert!(!rows.is_empty(), "no rows for {test}");
    let outcomes = run_all(&rows);
    let mut failed = String::new();
    for (row, outcome) in rows.iter().zip(&outcomes) {
        println!("{}", row.line(outcome));
        assert_eq!(outcome.underruns, 0, "{}: the line model ran dry", row.label);
        if let Err(why) = row.verdict(outcome) {
            let _ = writeln!(failed, "  {}: {why}", row.label);
        }
    }
    assert!(failed.is_empty(), "{test}:\n{failed}");
}

// ---------------------------------------------------------------------------
// The lines of §9.2
// ---------------------------------------------------------------------------

fn call(speed: Speed, line: Line, data_s: f64, seed: u64) -> Call {
    Call { offer: speed.offer(), speed, line, data_s, seed, trace: false }
}

/// The same impairments both ways.
fn both_ways(direction: Direction) -> Line {
    Line::TwoWire { to_calling: direction.clone(), to_answering: direction, hybrid: None }
}

/// `slips` in a random order, spread over `seconds`: one to each equal slot of
/// it, somewhere in its middle, so that no two are within three seconds of
/// each other and what each costs can be told apart.
fn spread(rng: &mut Rng, mut slips: Vec<Slip>, seconds: f64) -> Vec<(f64, Slip)> {
    rng.shuffle(&mut slips);
    let slot = seconds / slips.len() as f64;
    slips
        .into_iter()
        .enumerate()
        .map(|(k, slip)| (k as f64 * slot + 1.0 + rng.unit() * (slot - 3.0).max(0.0), slip))
        .collect()
}

/// A 20 ms slip every three to five seconds for `seconds`: inserts and drops
/// in turn, and the inserts filled each way a jitter buffer fills them, in
/// turn.
fn concealment(rng: &mut Rng, seconds: f64) -> Vec<(f64, Slip)> {
    let fills = [Fill::Repeat, Fill::Comfort, Fill::Silence];
    let mut at = 0.0;
    let mut slips = Vec::new();
    for k in 0.. {
        at += 3.0 + 2.0 * rng.unit();
        if at >= seconds {
            break;
        }
        let slip = if k % 2 == 0 { Slip::Insert(PACKET, fills[(k / 2) % 3]) } else { Slip::Drop(PACKET) };
        slips.push((at, slip));
    }
    slips
}

/// Five samples dropped and five repeated.
fn one_sample_slips() -> Vec<Slip> {
    std::iter::repeat_n(Slip::Drop(1), 5).chain(std::iter::repeat_n(Slip::Insert(1, Fill::Repeat), 5)).collect()
}

fn every_row() -> Vec<Row> {
    let mut rows = Vec::new();
    let mut add = |test, label: String, call, pass| rows.push(Row { test, label, call, pass });
    let everything = rate_signal(Rates::between(4800, 14_400));

    // Each rate on its own, offered alone, on a direct line with a one-sample
    // delay at its working SNR: the floor under everything else.
    for (k, speed) in Speed::ALL.into_iter().enumerate() {
        let line = both_ways(Direction { snr_db: Some(speed.working_snr()), ..Direction::default() });
        add(
            "every_rate_holds_at_its_working_snr",
            speed.name().into(),
            call(speed, line, 30.0, 0x100 + k as u64),
            Pass::Ber,
        );
    }

    // Both directions shifted up, then both shifted down.
    for (k, speed) in [Speed::At4800, Speed::At9600T, Speed::At14400T].into_iter().enumerate() {
        for hz in [7.0, -7.0] {
            let line =
                both_ways(Direction { carrier_hz: hz, snr_db: Some(speed.working_snr()), ..Direction::default() });
            add(
                "seven_hertz_either_way",
                format!("{hz:+.0} Hz {}", speed.name()),
                call(speed, line, 30.0, 0x200 + k as u64),
                Pass::Ber,
            );
        }
    }

    // The far end's clock 200 ppm fast one way and 200 ppm slow the other.
    for (k, speed) in [Speed::At4800, Speed::At14400T].into_iter().enumerate() {
        let direction = |ppm| Direction { ppm, snr_db: Some(speed.working_snr()), ..Direction::default() };
        let line = Line::TwoWire { to_calling: direction(-200.0), to_answering: direction(200.0), hybrid: None };
        add(
            "two_hundred_ppm_either_way",
            format!("±200 ppm {}", speed.name()),
            call(speed, line, 30.0, 0x300 + k as u64),
            Pass::Ber,
        );
    }

    // Ten one-sample slips each way at random moments in a minute of data, 3 dB
    // above the working SNR.
    for (k, speed) in [Speed::At4800, Speed::At9600T, Speed::At14400T].into_iter().enumerate() {
        let mut rng = Rng::new(0x400 + k as u64);
        let mut direction = || Direction {
            delay: 8,
            snr_db: Some(speed.working_snr() + 3.0),
            slips_in_data: spread(&mut rng, one_sample_slips(), 60.0),
            ..Direction::default()
        };
        let line = Line::TwoWire { to_calling: direction(), to_answering: direction(), hybrid: None };
        add("single_sample_slips", speed.name().into(), call(speed, line, 60.0, 0x400 + k as u64), Pass::Slips(0.97));
    }

    // A jitter buffer slipping 20 ms every three to five seconds each way, for
    // a minute of data.
    for (k, speed) in [Speed::At14400T, Speed::At9600T].into_iter().enumerate() {
        let mut rng = Rng::new(0x500 + k as u64);
        let mut direction =
            || Direction { delay: 16, slips_in_data: concealment(&mut rng, 60.0), ..Direction::default() };
        let line = Line::TwoWire { to_calling: direction(), to_answering: direction(), hybrid: None };
        add("concealment_slips", speed.name().into(), call(speed, line, 60.0, 0x500 + k as u64), Pass::Clean(0.97));
    }

    // A 3 dB step up and one back down, a 6 dB ramp up over half a second and
    // one back down, in a minute of data; and a softphone's limiter set off in
    // the start-up. The answering end's changes come a second after the
    // calling end's.
    for (k, speed) in [Speed::At14400T, Speed::At9600T].into_iter().enumerate() {
        let direction = |offset: f64| Direction {
            gains_in_data: vec![
                (8.0 + offset, 0.0, 3.0),
                (20.0 + offset, 0.0, 0.0),
                (32.0 + offset, 0.5, 6.0),
                (44.0 + offset, 0.5, 0.0),
            ],
            limiter: true,
            ..Direction::default()
        };
        let line = Line::TwoWire { to_calling: direction(0.0), to_answering: direction(1.0), hybrid: None };
        add("gain_steps_and_ramps", speed.name().into(), call(speed, line, 60.0, 0x600 + k as u64), Pass::Clean(0.99));
    }

    // The sound card loopback with its two clocks apart, offering everything.
    for (k, ppm) in [0.0, 5.0, 20.0, 100.0].into_iter().enumerate() {
        let line = Line::Cable { ppm, slips_in_data: Vec::new() };
        let call =
            Call { offer: everything, speed: Speed::At14400T, line, data_s: 60.0, seed: 0x700 + k as u64, trace: false };
        add("cable_with_drift", format!("cable {ppm} ppm"), call, Pass::Ber);
    }

    // The same card at 20 ppm dropping five samples, repeating five and
    // running dry twice for 160, in a minute of data.
    {
        let mut rng = Rng::new(0x800);
        let mut slips = one_sample_slips();
        slips.extend(std::iter::repeat_n(Slip::Insert(160, Fill::Silence), 2));
        let line = Line::Cable { ppm: 20.0, slips_in_data: spread(&mut rng, slips, 60.0) };
        let call = Call { offer: everything, speed: Speed::At14400T, line, data_s: 60.0, seed: 0x800, trace: false };
        add("cable_with_slips", "cable 20 ppm, slips".into(), call, Pass::Clean(0.97));
    }

    // Rory's line: 0.7 s each way, the two clocks 100 ppm apart, the
    // softphone's limiter on what the calling end hears, the working SNR, and
    // the jitter buffers slipping 20 ms every three to five seconds each way
    // from the moment the call starts.
    for (k, speed) in [Speed::At14400T, Speed::At9600T].into_iter().enumerate() {
        let mut rng = Rng::new(0x900 + k as u64);
        let mut direction = |ppm, limiter| Direction {
            ppm,
            delay: (0.7 * FS) as usize,
            snr_db: Some(speed.working_snr()),
            slips_from_start: concealment(&mut rng, 3.0 * PATIENCE_S),
            limiter,
            ..Direction::default()
        };
        let line = Line::TwoWire { to_calling: direction(-100.0, true), to_answering: direction(100.0, false), hybrid: None };
        add("rorys_voip_line", speed.name().into(), call(speed, line, 60.0, 0x900 + k as u64), Pass::Clean(0.95));
    }

    // `v32_call.rs`'s line with length -- a hybrid at each end, 20 ms each way,
    // the far hybrid's reflection a round trip later -- with the trunk
    // shifting the carrier 3 Hz and the two clocks 200 ppm apart.
    {
        let speed = Speed::At9600T;
        let delay = 320;
        let direction = |ppm| Direction {
            attenuation: FAR,
            carrier_hz: 3.0,
            ppm,
            delay,
            snr_db: Some(speed.working_snr()),
            ..Direction::default()
        };
        let line =
            Line::TwoWire { to_calling: direction(-200.0), to_answering: direction(200.0), hybrid: Some(2 * delay) };
        add("hybrid_with_everything", speed.name().into(), call(speed, line, 30.0, 0xA00), Pass::Held);
    }

    // Loud noise in place of the far end, both ways, for a second and a half,
    // four seconds into data at 14 400.
    {
        let line = both_ways(Direction { loss_in_data: Some((4.0, 1.5)), ..Direction::default() });
        let call = Call { offer: everything, speed: Speed::At14400T, line, data_s: 30.0, seed: 0xB00, trace: false };
        add("a_real_loss_still_retrains", "14400T, 1.5 s lost".into(), call, Pass::OneRetrain(14_400));
    }

    rows
}

// ---------------------------------------------------------------------------
// The tests
// ---------------------------------------------------------------------------

/// Each rate, offered alone, holds at its working SNR with no more than one
/// bit in 100 000 wrong.
///
/// The working SNR is where the line still ought to carry the rate: 3 dB above
/// the worse of the ideal decoder's 1e-5 point and where the retrain rule
/// gives up. A receiver that cannot hold there is losing more than its
/// arithmetic allows, and everything below is built on it. 9600 without the
/// trellis code is reached by V.32's own table.
#[test]
fn every_rate_holds_at_its_working_snr() {
    check("every_rate_holds_at_its_working_snr");
}

/// Seven hertz of carrier offset, both ways up and then both ways down, at
/// 4800, 9600 and 14 400.
///
/// V.32 2.1 and V.32bis 2.1 both require the receiver to "operate with
/// received frequency offsets of up to ± 7 Hz". A call starts at four points,
/// where a carrier loop has 45° to spare, and has to be locked tightly enough
/// by the time it reaches a hundred and twenty-eight.
#[test]
fn seven_hertz_either_way() {
    check("seven_hertz_either_way");
}

/// Two hundred parts per million between the two ends' clocks, which is two
/// conforming modems as far apart as they may be (V.32 2.3: 2400 baud to
/// within 0.01 %).
#[test]
fn two_hundred_ppm_either_way() {
    check("two_hundred_ppm_either_way");
}

/// A sample dropped or repeated costs tens of symbols, not a retrain.
///
/// One sample is a 40.5° turn of the carrier and a sixth of a symbol of
/// timing. Nothing in a call avoids them -- a sound card drops one, a buffer
/// underruns by one -- and a receiver that answers each with a retrain, and
/// the retrain with a lower rate, ends a call at 4800 for nothing the line
/// did wrong. Each slip may cost at most 200 ms of errored blocks, and 97 % of
/// blocks must be clean.
#[test]
fn single_sample_slips() {
    check("single_sample_slips");
}

/// A jitter buffer's 20 ms slips cost only the data they land on.
///
/// Twenty milliseconds is forty-eight symbols and thirty-six turns of 1800 Hz
/// exactly, so a slip is invisible to the loops; what fills an inserted one is
/// a stale repeat, comfort noise or silence, and a dropped one is a jump in
/// the data. Either way V.42 sends the frame again and the call goes on.
#[test]
fn concealment_slips() {
    check("concealment_slips");
}

/// Level changes, stepped and ramped, and a softphone's limiter set off in the
/// start-up, are followed without losing the call: at least 99 % of blocks
/// clean.
#[test]
fn gain_steps_and_ramps() {
    check("gain_steps_and_ramps");
}

/// A sound card loopback whose two clocks differ holds 14 400 cleanly.
///
/// The cable returns this end's own signal at full strength, so it is the
/// echo canceller that has to follow the drift; on a direct line the same
/// ppm is harmless. Contract experiment F: today 5 ppm is four retrains and
/// 20 ppm a call not connected at the end.
#[test]
fn cable_with_drift() {
    check("cable_with_drift");
}

/// The same loopback dropping, repeating and running dry, which is what
/// `modem-loop` counts as samples lost coming in and as underruns.
#[test]
#[ignore = "V.32 rebuild: enabled by package D"]
fn cable_with_slips() {
    check("cable_with_slips");
}

/// The line Rory's calls are made over: long, slipping, gain-controlled and
/// noisy, all at once and from the first sample. It has to hold for a minute
/// with 95 % of blocks clean.
#[test]
fn rorys_voip_line() {
    check("rorys_voip_line");
}

/// A hybrid at each end, a reflection off the far one, a trunk that shifts the
/// carrier, and two clocks apart, together at 9600.
#[test]
fn hybrid_with_everything() {
    check("hybrid_with_everything");
}

/// A real loss is still retrained: once, within 1.6 s of it, and back at the
/// rate the call had.
///
/// Holding on must not turn into never letting go. A second and a half of
/// loud noise where the far end was is a call that has lost its line, and 7's
/// retrain is the answer to it -- but a line that was lost has not been shown
/// unable to carry the rate, so the offer stays whole and the call comes back
/// at 14 400.
///
/// The noise is both ways, as a fault on the line is. Taken one way only, the
/// retrain starts a second into it and the answering modem's AC-to-CA
/// reversal falls inside what is left, where the calling modem cannot hear
/// it; both then wait for each other for the start-up's whole minute of
/// patience, which is the start-up's business and not the receiver's.
#[test]
fn a_real_loss_still_retrains() {
    check("a_real_loss_still_retrains");
}

/// Every line above, as the before (or after) table: the rate both ends came
/// up at, where each ended, retrains at each end, the share of errored blocks
/// at each end, the worse BER, and the median reception (residual over point
/// spacing) at each end.
#[test]
#[ignore = "a measurement: every line of §9.2"]
fn before_and_after_table() {
    let rows = every_row();
    let outcomes = run_all(&rows);
    println!(
        "| test | line | came up | ended (calling / answering) | retrains | errored blocks | worse BER | reception | verdict |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for (row, outcome) in rows.iter().zip(&outcomes) {
        println!("{}", row.line(outcome));
        if outcome.underruns > 0 {
            println!("  ({} samples short on the line: the line model ran dry)", outcome.underruns);
        }
    }
}

/// The lines whose test and label contain `V32_LINE`, one at a time, with the
/// start-up's phases, every errored block and every slip printed.
#[test]
#[ignore = "a probe: set V32_LINE"]
fn one_line_traced() {
    let want = std::env::var("V32_LINE").unwrap_or_default();
    for mut row in every_row().into_iter().filter(|r| format!("{} {}", r.test, r.label).contains(&want)) {
        println!("{} {}", row.test, row.label);
        row.call.trace = true;
        let outcome = row.call.run();
        println!("{}", row.line(&outcome));
    }
}
