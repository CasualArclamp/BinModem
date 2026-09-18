//! A route between a V.90 digital modem and an analogue modem, simulated.
//!
//! The digital modem hands the network a level every 125 microseconds, and
//! the network carries it as a G.711 codeword -- so anything that is not a
//! codeword already is quantised to one. Towards the analogue modem the codec
//! turns codewords into a waveform through its reconstruction filter, and the
//! loop adds its noise; the other way the codec filters and samples the
//! analogue modem's waveform and quantises it. A T1 on the way can rob a bit
//! from every sixth octet, and a digital pad can scale every level.
//!
//! Two things a VoIP call adds are here too. The analogue modem's sound card
//! runs on its own clock, some tens of parts per million off the network's,
//! so every waveform is resampled between the two. And the jitter buffer in
//! the softphone now and then plays twenty milliseconds of made-up audio, or
//! drops twenty, which shifts everything after it by 160 codewords.
//!
//! V.92 sends PCM upstream as well, so the upstream half of the route has to
//! be modelled as closely as the downstream half. The codec's A/D samples on
//! the network's own clock -- 6.2/V.92: "The upstream symbol rate shall be
//! 8000 symbol/s derived from the digital network" -- and nothing at the far
//! end can move its sampling instants: 8.6.3/V.92, "The digital modem is not
//! capable of changing the sampling phase of the central office A/D. Hence,
//! it shall use signal Jp to indicate its desire to the analogue modem to
//! adjust its transmitter phase". So the A/D here has a sampling phase of its
//! own, and each direction has its own delay, noise, pad, robbed bit and
//! slips; the upstream also gives back the codeword it made, not only the
//! level, because a V.92 receiver thinks in Ucodes and signs.
//!
//! The anti-alias filter in front of that A/D is the most expensive thing in
//! the model: a 257-tap windowed sinc for every network sample. Its taps
//! depend only on where the sampling instant falls between two line samples,
//! so they are tabulated for that phase and rebuilt only when it moves --
//! which, the clocks being the same, it never does. Two clocks that differ
//! move it every sample, and then the table costs what the computation it
//! replaces cost; `tabulate_up_kernel` says why it is not rounded to a grid
//! to get out of that.
//!
//! Three more things a V.92 call meets are modelled here, and all three are
//! per-direction, so the settings of one leg live in `Impairments` and the
//! route holds two of them.
//!
//! The first is echo. 1 b)/V.92 lists "channel separation by echo
//! cancellation techniques" among the principal characteristics of these
//! modems, and 9.8 says a rate renegotiation "can also be used to retrain the
//! analogue modem's echo canceller or the precoder and prefilter without going
//! through a complete retrain" -- so a silent period exists for a canceller to
//! train in, and a route with no echo in it never asks for one. The hybrid at
//! the central office leaks each direction into the other: a short filter of
//! what the digital modem sent arrives at the A/D, and a short filter of what
//! the A/D made goes back down the loop. A far echo, one VoIP round trip late,
//! is one more tap a long way back.
//!
//! The second is a transcoding gateway: a leg whose codewords are decoded,
//! low-passed and re-encoded in the other law. The Crazytel path was measured
//! doing exactly that, and its low-pass is 3 dB down at 3750 Hz and 18 dB down
//! at 4000 Hz -- against a band edge the upstream constellation sits right on.
//!
//! The third is that with both ends of a call ours, over a softphone, there
//! is no analogue loop upstream at all: our samples go into a G.711 encoder,
//! either one in two exactly or through a resampler, and the packets carry them
//! verbatim. `UpPath` says which, and on those paths a clock offset is not a
//! resampling but a slip every so often in the far end's jitter buffer.
//!
//! Nothing here is a claim about any real network, only about what V.90 and
//! V.92 have to get through.

use std::collections::VecDeque;
use std::f64::consts::PI;

use super::ucode::{self, Law};

/// The network's rate.
const NETWORK_FS: f64 = 8000.0;

/// Codewords either side of an instant the codec's reconstruction reaches:
/// short, since a reconstruction filter that rings on for longer than an
/// equaliser reaches is not one any codec has.
const DOWN_REACH: i64 = 20;

/// Line samples either side the codec's anti-alias filter reaches, in
/// seconds of line: long, since the upstream's band runs close to 4 kHz.
const UP_REACH: f64 = 0.008;

/// A slip's length, in codewords: one twenty-millisecond packet, and the
/// default for either direction.
pub const SLIP: usize = 160;

/// The upstream anti-alias filter's cutoff, in hertz, unless one is asked for.
const UP_CUTOFF: f64 = 3700.0;

/// How far behind the newest line sample the codec's A/D takes its sample, in
/// codewords: late enough that the analogue modem has said everything the
/// filter reaches, since what it says answers a downstream that is itself
/// late by the reconstruction's reach.
const UP_LAG: f64 = DOWN_REACH as f64 + 2.0 + UP_REACH * NETWORK_FS;

/// The rate a softphone resamples through on its way to its encoder.
const SOFTPHONE_FS: f64 = 48_000.0;

/// How many points a transcoding gateway's low-pass is designed on.
///
/// The taps come out as the inverse transform of the wanted response sampled
/// every `NETWORK_FS / TRANSCODER_GRID` hertz, which is 250 Hz here, so the
/// filter's gain is exactly that response at every multiple of 250 Hz and
/// within 0.2 dB of it in between. Thirty-two puts both of the frequencies the
/// Crazytel path was measured at on the grid, and costs 33 taps: sixteen
/// codewords, two milliseconds, of delay through the gateway.
const TRANSCODER_GRID: usize = 32;

/// A transcoding gateway's low-pass, as the frequency where it starts to roll
/// off and the frequency where it has reached nothing.
///
/// The measurement is the project's own, on the Crazytel path: mu-law decoded,
/// low-passed 3 dB down at 3750 Hz and 18 dB down at 4000 Hz, and re-encoded
/// as A-law. A raised cosine in amplitude through both of those points starts
/// at 3525.98 Hz and ends at 4142.32 Hz -- 10^(-3/20) and 10^(-18/20) are
/// 0.363 and 0.769 of the way through such a roll-off, which fixes its width
/// at 250 Hz / (0.769 - 0.363) and then its start. The shape between the two
/// measured points is a guess; the two points are not.
pub const CRAZYTEL_LOW_PASS: (f64, f64) = (3525.98, 4142.32);

/// Which way round a leg of the route runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Digital modem to analogue modem.
    Down,
    /// Analogue modem to digital modem.
    Up,
}

/// How the analogue modem's samples reach the codec that makes codewords of
/// them.
///
/// A telephone loop is one answer and the only one V.90 needed. With both ends
/// of the call ours over VoIP there is no loop: our sound card's samples are
/// decimated inside the softphone and handed to its G.711 encoder, and which
/// of the two things it does with them decides whether PCM upstream is
/// possible at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpPath {
    /// A telephone loop: the codec's anti-alias filter, its own sampling
    /// instant, and the network's clock. The default, and what V.90 is tested
    /// over.
    Loop,
    /// A softphone that hands its encoder one line sample in `fs / 8000`,
    /// counting from `phase`, and nothing else: no filter, no interpolation,
    /// so the encoder sees the sample the modem wrote. Only one of the phases
    /// carries codewords.
    Straight {
        /// Which of the `fs / 8000` line samples the encoder is given.
        phase: usize,
    },
    /// A softphone that resamples on the way: up to 48 kHz, `delay` samples of
    /// buffer there, and down to the network's 8 kHz. Nothing survives that as
    /// a codeword.
    Resampled {
        /// Samples of buffer at 48 kHz.
        delay: usize,
    },
}

/// What of the other direction comes back into this one: how far down it is,
/// and the shape it comes back with, a tap to the codeword.
#[derive(Debug, Clone)]
struct Echo {
    gain: f64,
    taps: Vec<f64>,
}

/// A gateway at the end of a leg: it decodes what the leg carried, low-passes
/// it and encodes it again in another law.
#[derive(Debug, Clone)]
struct Transcoder {
    to: Law,
    taps: Vec<f64>,
    history: VecDeque<f64>,
}

impl Transcoder {
    fn new(to: Law, low_pass: Option<(f64, f64)>) -> Self {
        let taps = low_pass.map(|(start, end)| low_pass_taps(start, end)).unwrap_or_default();
        let history = VecDeque::from(vec![0.0; taps.len()]);
        Self { to, taps, history }
    }

    /// One codeword through the gateway, and the level the other side of it
    /// has: still a codeword, but of the other law and of a filtered signal,
    /// which is why a transcoded leg cannot carry what we meant.
    fn carry(&mut self, level: f64) -> f64 {
        let filtered = if self.taps.is_empty() {
            level
        } else {
            self.history.pop_front();
            self.history.push_back(level);
            self.taps.iter().zip(self.history.iter()).map(|(t, x)| t * x).sum()
        };
        quantise(self.to, filtered)
    }
}

/// The softphone that resamples: the analogue modem's rate up to 48 kHz, a
/// few samples of buffer, and 48 kHz down to the network's own rate.
#[derive(Debug, Clone)]
struct Resampled {
    raised: dsp::Resampler,
    buffer: VecDeque<f64>,
    lowered: dsp::Resampler,
    high: Vec<f64>,
    low: Vec<f64>,
    out: VecDeque<f64>,
}

impl Resampled {
    fn new(fs: f64, delay: usize) -> Self {
        Self {
            raised: dsp::Resampler::new(fs, SOFTPHONE_FS),
            buffer: VecDeque::from(vec![0.0; delay]),
            lowered: dsp::Resampler::new(SOFTPHONE_FS, NETWORK_FS),
            high: Vec::new(),
            low: Vec::new(),
            out: VecDeque::new(),
        }
    }

    /// One line sample in, and whatever codeword-rate samples it completes
    /// waiting in `out`.
    fn feed(&mut self, x: f64) {
        self.high.clear();
        self.raised.process(x, &mut self.high);
        for &h in &self.high {
            self.buffer.push_back(h);
            let Some(late) = self.buffer.pop_front() else { continue };
            self.low.clear();
            self.lowered.process(late, &mut self.low);
            self.out.extend(self.low.iter().copied());
        }
    }
}

/// What one leg of the route does to the codewords it carries.
///
/// The two legs of a V.92 call are the same kind of thing -- each has its own
/// robbed bit, its own pad, its own gateway, its own limiter, its own share of
/// the hybrid's echo and its own jitter buffer -- so this is written once and
/// the route holds two of them. What is *not* here belongs to the route rather
/// than to a leg: the loop's noise, which is added to an analogue waveform and
/// not to a codeword; the delays; a slip's length, which is a packet either way
/// round; and the sample buffers themselves.
#[derive(Debug, Clone)]
struct Impairments {
    /// Which of the six octets a robbed bit lands on, if one does, and how
    /// many have gone by.
    robbed: Option<usize>,
    octets: usize,
    /// A digital pad, as a gain on every level.
    pad: f64,
    /// A gateway at the far end of the leg, if there is one.
    transcoder: Option<Transcoder>,
    /// A limiter on the waveform: the loudest it lets through, how fast it
    /// recovers, in seconds, and where its gain has got to.
    gain_control: Option<(f64, f64)>,
    gain: f64,
    /// What of the other direction leaks into this one at the hybrid, and one
    /// tap of it a whole round trip later.
    echo: Option<Echo>,
    far_echo: Option<(f64, usize)>,
    /// Slips: how often, and whether audio is made up or lost; or one, at a
    /// given codeword; and how many have happened.
    slips: Option<(u64, bool)>,
    slip_at: Option<(u64, bool)>,
    slip_count: u32,
    /// Codewords of a lost stretch still to drop, and the last slip's worth of
    /// codewords for concealment to repeat.
    dropping: usize,
    recent: VecDeque<f64>,
}

impl Impairments {
    fn new() -> Self {
        Self {
            robbed: None,
            octets: 0,
            pad: 1.0,
            transcoder: None,
            gain_control: None,
            gain: 1.0,
            echo: None,
            far_echo: None,
            slips: None,
            slip_at: None,
            slip_count: 0,
            dropping: 0,
            recent: VecDeque::with_capacity(SLIP),
        }
    }

    /// What the leg makes of one level: the nearest codeword, through its
    /// robbed bit and its pad, and then through whatever gateway ends it.
    ///
    /// The codeword comes back as well as the level, because a V.92 receiver
    /// decides on codewords and A-law has no exact zero, and it is the one the
    /// far end reads: on a transcoded leg that is not the one that went in.
    fn carry(&mut self, law: Law, level: f64) -> (f64, (u8, bool)) {
        let (u, negative) = ucode::nearest(law, (level * 32768.0).round() as i32);
        let (u, negative) = match self.robbed {
            Some(phase) => {
                let mut octet = ucode::octet(law, u, negative);
                if phase == self.octets % 6 {
                    octet |= 1;
                }
                ucode::from_octet(law, octet)
            }
            None => (u, negative),
        };
        self.octets += 1;
        let level = ucode::level(law, u) * if negative { -1.0 } else { 1.0 };
        let (level, u, negative) = if self.pad == 1.0 {
            (level, u, negative)
        } else {
            let (u, negative) = ucode::nearest(law, (level * self.pad * 32768.0).round() as i32);
            (ucode::level(law, u) * if negative { -1.0 } else { 1.0 }, u, negative)
        };
        match &mut self.transcoder {
            // A gateway hands on a level of its own law, so which codeword
            // that is has to be asked again.
            Some(gateway) => {
                let level = gateway.carry(level);
                let (u, negative) = ucode::nearest(law, (level * 32768.0).round() as i32);
                (level, (u, !negative))
            }
            None => (level, (u, !negative)),
        }
    }

    /// One analogue sample through the leg's gain control: anything louder
    /// than the ceiling is turned down to it at once, and the gain comes back
    /// up over the release. Without one the sample is the sample.
    fn limit(&mut self, x: f64, fs: f64) -> f64 {
        let Some((ceiling, release)) = self.gain_control else { return x };
        if (x * self.gain).abs() > ceiling {
            self.gain = ceiling / x.abs();
        }
        let out = x * self.gain;
        self.gain += (1.0 - self.gain) / (release * fs);
        out
    }

    /// How far back this leg's echoes reach into what the other one carried,
    /// in codewords: nothing at all unless one was asked for.
    fn echo_reach(&self) -> usize {
        let hybrid = self.echo.as_ref().map_or(0, |echo| echo.taps.len());
        let far = self.far_echo.map_or(0, |(_, delay)| delay + 1);
        hybrid.max(far)
    }

    /// What of the other direction is in this leg's signal: the hybrid's short
    /// filter of what that direction carried, newest codeword first, and the
    /// far end's one tap a round trip back.
    fn echo_of(&self, carried: &VecDeque<f64>) -> Option<f64> {
        if self.echo.is_none() && self.far_echo.is_none() {
            return None;
        }
        let back = |k: usize| carried.len().checked_sub(1 + k).and_then(|i| carried.get(i)).copied();
        let mut sum = 0.0;
        if let Some(echo) = &self.echo {
            for (k, tap) in echo.taps.iter().enumerate() {
                sum += echo.gain * tap * back(k).unwrap_or(0.0);
            }
        }
        if let Some((gain, delay)) = self.far_echo {
            sum += gain * back(delay).unwrap_or(0.0);
        }
        Some(sum)
    }

    /// A level on its way into the buffer the far end plays out of: thrown
    /// away while a lost stretch is still running, and kept either way for
    /// concealment to repeat.
    fn buffer(&mut self, level: f64, length: usize, into: &mut VecDeque<f64>) {
        if self.dropping > 0 {
            self.dropping -= 1;
            return;
        }
        into.push_back(level);
        if self.recent.len() == length {
            self.recent.pop_front();
        }
        self.recent.push_back(level);
    }

    /// That buffer slipping: a slip's worth of the last slip's worth again,
    /// fading, which is what packet loss concealment makes up -- or a slip's
    /// worth of what is coming thrown away.
    fn conceal(&mut self, inserted: bool, length: usize, into: &mut VecDeque<f64>) {
        self.slip_count += 1;
        if inserted {
            let held = self.recent.len() as f64;
            for (k, v) in self.recent.iter().enumerate() {
                into.push_back(v * (1.0 - k as f64 / held));
            }
        } else {
            self.dropping = length;
        }
    }
}

/// A route. The analogue side runs at `fs`.
#[derive(Debug, Clone)]
pub struct Network {
    law: Law,
    fs: f64,
    /// How far the analogue modem's clock is off the network's, as a
    /// fraction: its samples are `(1 + skew) / fs` apart in network time.
    skew: f64,
    /// Downstream: carried codewords with the network time of the first, and
    /// where the next analogue sample falls.
    down_levels: VecDeque<f64>,
    down_first: f64,
    down_next: f64,
    down_delay: f64,
    /// Upstream: the analogue modem's samples with the analogue time of the
    /// first, and the next network sample's index.
    up_samples: VecDeque<f64>,
    up_first: f64,
    up_next: f64,
    up_delay: f64,
    /// Network time: codewords sent downstream so far, and upstream.
    now: u64,
    up_now: u64,
    noise: f64,
    /// The loop's noise the other way, when it is not the same noise.
    up_noise: Option<f64>,
    seed: u64,
    /// What each leg of the route does to what it carries.
    down: Impairments,
    up: Impairments,
    /// Whether the upstream is quantised to G.711.
    quantised: bool,
    /// What the analogue modem's line level is to the codec's full scale.
    ///
    /// Every modem here leaves at a root-mean-square of 0.707, a full-scale
    /// sine, and a codec quantising that clips every peak. A telephone line
    /// delivers a modem's -9 to -12 dBm to the codec well inside its range;
    /// this is where that happens.
    up_gain: f64,
    /// The codec's A/D sampling instant, as a fraction of a symbol after the
    /// analogue modem's own sample instants.
    up_phase: f64,
    /// The upstream anti-alias filter's cutoff, in hertz.
    up_cutoff: f64,
    /// The last codeword the A/D made: its Ucode, and whether it was positive.
    up_last: (u8, bool),
    /// The anti-alias filter's taps, and the sampling phase they were built
    /// for, as that fraction's bit pattern.
    up_kernel: Vec<f64>,
    up_kernel_at: Option<u64>,
    /// How long a slip is, in codewords: a packet, either way round.
    slip_length: usize,
    /// How far the far end's buffer has moved our upstream, in codewords:
    /// positive is later. The loop path's own way of slipping.
    up_shift: f64,
    /// How the analogue modem's samples reach the codec, the resampler chain
    /// one of the answers needs, and how many line samples have gone by, for
    /// the one that counts them.
    up_path: UpPath,
    resampled: Option<Resampled>,
    line_count: u64,
    /// What a softphone path has handed the encoder and not yet been asked
    /// for: the far end's jitter buffer, which starts out holding a packet
    /// because a buffer holding nothing cannot give one up.
    to_codec: VecDeque<f64>,
    codec_primed: bool,
    /// The levels each leg carried lately, for the other leg's echo to be a
    /// filter of: kept only as far back as that echo reaches.
    sent_down: VecDeque<f64>,
    sent_up: VecDeque<f64>,
}

impl Network {
    pub fn new(law: Law, fs: f64) -> Self {
        Self {
            law,
            fs,
            skew: 0.0,
            down_levels: VecDeque::new(),
            down_first: 0.0,
            down_next: 0.0,
            down_delay: 0.0,
            up_samples: VecDeque::new(),
            up_first: 0.0,
            up_next: 0.0,
            up_delay: 0.0,
            now: 0,
            up_now: 0,
            noise: 0.0,
            up_noise: None,
            seed: 0x2545_f491_4f6c_dd1d,
            down: Impairments::new(),
            up: Impairments::new(),
            quantised: true,
            up_gain: 0.25,
            up_phase: 0.0,
            up_cutoff: UP_CUTOFF,
            up_last: (0, true),
            up_kernel: Vec::new(),
            up_kernel_at: None,
            slip_length: SLIP,
            up_shift: 0.0,
            up_path: UpPath::Loop,
            resampled: None,
            line_count: 0,
            to_codec: VecDeque::new(),
            codec_primed: false,
            sent_down: VecDeque::new(),
            sent_up: VecDeque::new(),
        }
    }

    /// Each way's delay, in seconds of line: the older spelling of
    /// `with_delays`, which sets both legs at once and so replaces whatever
    /// that gave either of them.
    pub fn with_delay(self, seconds: f64, _fs: f64) -> Self {
        self.with_delays(seconds, seconds)
    }

    /// Each leg's delay on its own, in seconds of line: what a call whose two
    /// halves take different routes has, and what makes the round trip longer
    /// than twice either leg.
    pub fn with_delays(mut self, down: f64, up: f64) -> Self {
        self.down_delay = down;
        self.up_delay = up;
        self
    }

    /// The analogue modem's clock `ppm` parts per million fast.
    ///
    /// On a loop that is a resampling both ways, because the far codec samples
    /// our waveform on the network's clock. On a softphone path the upstream
    /// is not resampled at all and the difference piles up in the far jitter
    /// buffer instead; `with_up_path` says what that comes to.
    pub fn with_clock(mut self, ppm: f64) -> Self {
        self.skew = -ppm * 1e-6;
        self
    }

    /// Noise on the loop, as a level: one is full scale.
    pub fn with_noise(mut self, level: f64) -> Self {
        self.noise = level;
        self
    }

    /// Noise on the upstream loop alone, as a level: the two directions are
    /// different questions, and a margin measured one way says nothing about
    /// the other. Without this both use `with_noise`.
    pub fn with_upstream_noise(mut self, level: f64) -> Self {
        self.up_noise = Some(level);
        self
    }

    /// The loop's noise from now on: a line that goes bad in the middle of a
    /// call.
    pub fn set_noise(&mut self, level: f64) {
        self.noise = level;
    }

    /// A robbed bit on every sixth downstream octet, starting at `phase`.
    pub fn with_robbed_bit(mut self, phase: usize) -> Self {
        self.down.robbed = Some(phase % 6);
        self
    }

    /// A robbed bit on every sixth upstream octet, starting at `phase`.
    ///
    /// A T1 robs bits in both directions, and the two phases have nothing to
    /// do with each other: the upstream octets are counted from the first
    /// codeword the A/D made, not from the first one sent down.
    pub fn with_upstream_robbed_bit(mut self, phase: usize) -> Self {
        self.up.robbed = Some(phase % 6);
        self
    }

    /// A digital pad of `db` on the downstream, and none on the upstream: the
    /// older spelling of `with_pads`, so it takes an upstream pad off again
    /// rather than leaving it where it was.
    pub fn with_pad(self, db: f64) -> Self {
        self.with_pads(db, 0.0)
    }

    /// A digital pad on each direction, in decibels: a scale applied to the
    /// codeword the route is carrying, and then requantised, since a pad in
    /// the network is a digital one and its output is a codeword again.
    pub fn with_pads(mut self, down_db: f64, up_db: f64) -> Self {
        self.down.pad = 10f64.powf(-down_db / 20.0);
        self.up.pad = 10f64.powf(-up_db / 20.0);
        self
    }

    /// A transcoding gateway on both legs: what it re-encodes in, and the
    /// low-pass it puts the signal through first, as the frequency where the
    /// roll-off begins and the frequency where it has reached nothing.
    ///
    /// `CRAZYTEL_LOW_PASS` is the one that was measured. A gateway with no
    /// low-pass at all still moves every codeword, because it re-quantises on
    /// the other law's grid.
    pub fn with_transcoder(self, to: Law, low_pass: Option<(f64, f64)>) -> Self {
        self.with_transcoder_in(Direction::Down, to, low_pass).with_transcoder_in(Direction::Up, to, low_pass)
    }

    /// A transcoding gateway on one leg. Only the downstream of the Crazytel
    /// path was ever measured, so which legs a real gateway touches is a
    /// question a test should be able to ask either way.
    pub fn with_transcoder_in(mut self, dir: Direction, to: Law, low_pass: Option<(f64, f64)>) -> Self {
        let gateway = Some(Transcoder::new(to, low_pass));
        match dir {
            Direction::Down => self.down.transcoder = gateway,
            Direction::Up => self.up.transcoder = gateway,
        }
        self
    }

    /// The hybrid at the central office, leaking each direction into the
    /// other: `hybrid_db` below what it leaks from, with the shape of `taps`,
    /// a tap to the codeword, and the same shape both ways round.
    ///
    /// 1 b)/V.92 gives "channel separation by echo cancellation techniques" as
    /// one of the principal characteristics of these modems, which is only a
    /// characteristic of a route that has an echo in it. The taps are scaled
    /// so that the largest of them is `hybrid_db` down, so a delay is leading
    /// zeros and the level is the level.
    ///
    /// Both sides of the hybrid are filters of what the route carried rather
    /// than of either waveform: the downstream's echo is added to the A/D's
    /// own sample, before the quantiser that is the reason a canceller can
    /// never quite undo it, and the upstream's is added to the downstream
    /// level before the codec reconstructs it. The second of those is a
    /// simplification -- a reflection down the loop never becomes a codeword
    /// -- and what it costs is that the analogue modem's own echo comes back
    /// band-limited to the A/D's edge.
    pub fn with_echo(mut self, hybrid_db: f64, taps: &[f64]) -> Self {
        let peak = taps.iter().fold(0f64, |peak, t| peak.max(t.abs()));
        let shape: Vec<f64> = if peak > 0.0 { taps.iter().map(|t| t / peak).collect() } else { Vec::new() };
        let echo = Some(Echo { gain: 10f64.powf(-hybrid_db / 20.0), taps: shape });
        self.down.echo = echo.clone();
        self.up.echo = echo;
        self
    }

    /// The far end's echo of what the analogue modem sent, `db` down and
    /// `delay` seconds late: what a live V.34 far end did, only 25 dB down,
    /// at a VoIP round trip's remove.
    pub fn with_far_echo(self, db: f64, delay: f64) -> Self {
        self.with_far_echo_in(Direction::Down, db, delay)
    }

    /// A far echo on one leg. The digital modem has the same problem the other
    /// way round when its own end of the call is a softphone: what comes back
    /// is the far hybrid seen through two jitter buffers.
    pub fn with_far_echo_in(mut self, dir: Direction, db: f64, delay: f64) -> Self {
        let echo = Some((10f64.powf(-db / 20.0), (delay * NETWORK_FS).round() as usize));
        match dir {
            Direction::Down => self.down.far_echo = echo,
            Direction::Up => self.up.far_echo = echo,
        }
        self
    }

    /// The analogue modem's level at the codec, against its own.
    pub fn with_upstream_gain(mut self, gain: f64) -> Self {
        self.up_gain = gain;
        self
    }

    /// The upstream anti-alias filter's cutoff, in hertz.
    ///
    /// The default, 3700 Hz, is a gentle one. A transcoding gateway is not:
    /// the Crazytel path measured -3 dB at 3.75 kHz and -18 dB at 4 kHz, which
    /// is `with_transcoder` and `CRAZYTEL_LOW_PASS`, and the upstream
    /// constellation lives right against that edge.
    ///
    /// This is the loop's own codec. A softphone path has no analogue loop and
    /// no anti-alias filter, so it takes no notice of this, nor of
    /// `with_upstream_phase`.
    pub fn with_upstream_cutoff(mut self, hz: f64) -> Self {
        self.up_cutoff = hz;
        self.up_kernel_at = None;
        self
    }

    /// Where the codec's A/D takes its sample, as a fraction of a symbol
    /// after the analogue modem's own sample instants.
    ///
    /// 8.6.3: "The digital modem is not capable of changing the sampling
    /// phase of the central office A/D. Hence, it shall use signal Jp to
    /// indicate its desire to the analogue modem to adjust its transmitter
    /// phase from [0, 1) symbol or [0, T) seconds" -- Table 22's bits 18:33,
    /// a 16-bit unsigned fraction of a symbol. With the default zero, an `fs`
    /// that is a multiple of 8000 and no skew, every A/D instant lands on a
    /// line sample, so that fraction would always come out zero and Su and Jp
    /// would never be exercised.
    ///
    /// A softphone path has no instant to fall between our samples: it hands
    /// the encoder a sample of ours, and `UpPath::Straight`'s own phase says
    /// which. This is the loop's.
    pub fn with_upstream_phase(mut self, fraction_of_t: f64) -> Self {
        self.up_phase = fraction_of_t;
        self
    }

    /// An upstream carried as it is, with no codec's quantising: for finding
    /// out what the quantising costs.
    pub fn unquantised(mut self) -> Self {
        self.quantised = false;
        self
    }

    /// A downstream slip every `seconds`: a slip's worth of audio made up if
    /// `inserted`, lost otherwise. The upstream keeps whatever `with_slips_in`
    /// gave it.
    pub fn with_slips(self, seconds: f64, inserted: bool) -> Self {
        self.with_slips_in(Direction::Down, seconds, inserted)
    }

    /// A slip in one direction every `seconds`: a slip's worth of audio made
    /// up if `inserted`, lost otherwise.
    pub fn with_slips_in(mut self, dir: Direction, seconds: f64, inserted: bool) -> Self {
        let every = Some(((seconds * NETWORK_FS) as u64, inserted));
        match dir {
            Direction::Down => self.down.slips = every,
            Direction::Up => self.up.slips = every,
        }
        self
    }

    /// A gain control on the downstream as the analogue modem hears it:
    /// anything louder than `ceiling` of full scale is turned down to it at
    /// once, and the gain comes back up over `release` seconds -- what a live
    /// call through a softphone did to codewords above about a third of full
    /// scale.
    pub fn with_gain_control(mut self, ceiling: f64, release: f64) -> Self {
        self.down.gain_control = Some((ceiling, release));
        self
    }

    /// The same gain control on the analogue modem's own samples, before they
    /// reach the codec: what its capture path would do to them if it has one.
    ///
    /// Only what a softphone *played* was ever measured, and it held codewords
    /// above about a third of full scale down to it and read the ones after
    /// low for a third of a second. What it does to what it captures is
    /// unknown, which is the reason to be able to ask: precoding raises the
    /// peak-to-average ratio, so the prefilter's output can go over a ceiling
    /// the constellation never reaches.
    pub fn with_upstream_gain_control(mut self, ceiling: f64, release: f64) -> Self {
        self.up.gain_control = Some((ceiling, release));
        self
    }

    /// How the analogue modem's samples reach the codec.
    ///
    /// A clock offset means different things on the two kinds of path. On a
    /// loop the far codec samples our waveform on the network's clock, so the
    /// offset is a resampling and `with_clock` is one. On a softphone path our
    /// samples are forwarded as they are, and the offset turns up at the far
    /// jitter buffer instead: it fills or empties by a packet every
    /// `slip_length / |offset|` codewords, which is one twenty-millisecond
    /// slip every 175 seconds at 114 ppm.
    pub fn with_up_path(mut self, path: UpPath) -> Self {
        self.resampled = match path {
            UpPath::Resampled { delay } => Some(Resampled::new(self.fs, delay)),
            _ => None,
        };
        self.up_path = path;
        self
    }

    /// One downstream slip, `seconds` into the call, leaving the upstream's
    /// own schedule where it was.
    pub fn with_slip_at(self, seconds: f64, inserted: bool) -> Self {
        self.with_slip_at_in(Direction::Down, seconds, inserted)
    }

    /// One slip in one direction, `seconds` into the call.
    pub fn with_slip_at_in(mut self, dir: Direction, seconds: f64, inserted: bool) -> Self {
        let at = Some(((seconds * NETWORK_FS) as u64, inserted));
        match dir {
            Direction::Down => self.down.slip_at = at,
            Direction::Up => self.up.slip_at = at,
        }
        self
    }

    /// How long a slip is, in codewords, in either direction.
    ///
    /// Twenty milliseconds, 160 codewords, is one packet and the default. The
    /// cuts heard on live calls are half that, and ten milliseconds matters
    /// because 160 moves a twelve-symbol upstream frame by four symbols and
    /// 80 moves it by eight (8.5.7: the digital modem keeps frame alignment
    /// from the first symbol of the second TRN1u, and a slip is what takes it
    /// away again).
    pub fn with_slip_length(mut self, codewords: usize) -> Self {
        self.slip_length = codewords;
        self
    }

    /// Downstream slips so far: the older spelling of `slips_down`, which the
    /// V.90 tests count with and which stays downstream-only, so that adding
    /// an upstream slip to one of them cannot change what it already asserts.
    pub fn slips(&self) -> u32 {
        self.down.slip_count
    }

    /// Downstream slips so far.
    pub fn slips_down(&self) -> u32 {
        self.down.slip_count
    }

    /// Upstream slips so far: only those the far end's buffer could make,
    /// whether they were asked for or fell out of a clock offset on a
    /// softphone path.
    pub fn slips_up(&self) -> u32 {
        self.up.slip_count
    }

    /// The last codeword the A/D made: its Ucode, and whether it was
    /// positive.
    ///
    /// A V.92 upstream receiver decides on codewords rather than on levels,
    /// and A-law has no exact zero, so a test that wants to know what arrived
    /// wants this rather than `up`.
    ///
    /// Under `unquantised` the A/D makes no codeword, and this is the one the
    /// level came nearest: still an answer about the last sample, never a
    /// stale one from before the quantiser was taken out. It has had neither
    /// the upstream robbed bit nor the upstream pad, because neither can be
    /// done to something that is not a codeword -- and neither has the level
    /// `up` returned.
    pub fn up_code(&self) -> (u8, bool) {
        self.up_last
    }

    fn gaussian(&mut self) -> f64 {
        let mut sum = 0.0;
        for _ in 0..4 {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 7;
            self.seed ^= self.seed << 17;
            sum += (self.seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
        }
        sum * 3f64.sqrt()
    }

    /// The nearest codeword of the route's law, as a level: what a test that
    /// works out what a leg should have made of something needs, now that the
    /// legs quantise for themselves.
    #[cfg(test)]
    fn quantise(&self, level: f64) -> f64 {
        quantise(self.law, level)
    }

    /// What the network makes of one downstream level: the nearest codeword,
    /// through whatever the downstream leg does to it.
    fn carry(&mut self, level: f64) -> f64 {
        self.down.carry(self.law, level).0
    }

    /// What the network makes of one upstream sample the A/D has taken: the
    /// nearest codeword, through the upstream leg, and remembered as a
    /// codeword for `up_code`.
    fn carry_up(&mut self, level: f64) -> f64 {
        let (level, code) = self.up.carry(self.law, level);
        self.up_last = code;
        level
    }

    /// The level a leg has just carried, kept for the other leg's echo to be
    /// a filter of, and only as far back as that echo reaches -- which is not
    /// at all unless one was asked for.
    fn remember(&mut self, dir: Direction, level: f64) {
        let (reach, carried) = match dir {
            Direction::Down => (self.up.echo_reach(), &mut self.sent_down),
            Direction::Up => (self.down.echo_reach(), &mut self.sent_up),
        };
        if reach == 0 {
            return;
        }
        if carried.len() == reach {
            carried.pop_front();
        }
        carried.push_back(level);
    }

    /// One downstream level in, and whatever line samples the analogue modem
    /// hears by then out.
    pub fn down(&mut self, level: f64) -> Vec<f64> {
        let carried = self.carry(level);
        self.remember(Direction::Down, carried);
        // What the hybrid sends back down the loop, so that the analogue
        // modem hears its own upstream: the echo its own canceller is for.
        let carried = match self.down.echo_of(&self.sent_up) {
            Some(echo) => carried + echo,
            None => carried,
        };
        self.now += 1;
        // The jitter buffer, between the network and the sound card.
        let periodic = self.down.slips.filter(|(every, _)| self.now.is_multiple_of(*every));
        let once = self.down.slip_at.filter(|(at, _)| self.now == *at);
        if let Some((_, inserted)) = periodic.or(once) {
            self.down.conceal(inserted, self.slip_length, &mut self.down_levels);
        }
        self.down.buffer(carried, self.slip_length, &mut self.down_levels);
        let mut out = Vec::new();
        if self.down_delay > 0.0 {
            // The delay, as silence first.
            out.extend(std::iter::repeat_n(0.0, (self.down_delay * self.fs).round() as usize));
            self.down_delay = 0.0;
        }
        // Line samples up to where the reconstruction has everything it
        // needs, in the buffer's own time.
        let step = (1.0 + self.skew) * NETWORK_FS / self.fs;
        let last = self.down_first + self.down_levels.len() as f64 - 1.0;
        while self.down_next + DOWN_REACH as f64 <= last {
            let t = self.down_next;
            let centre = t.floor() as i64;
            let mut sum = 0.0;
            for j in centre - DOWN_REACH..=centre + DOWN_REACH {
                let index = j as f64 - self.down_first;
                if index < 0.0 {
                    continue;
                }
                let Some(&v) = self.down_levels.get(index as usize) else { continue };
                sum += v * kernel(t - j as f64, 3800.0 / NETWORK_FS, DOWN_REACH as f64 + 1.0);
            }
            let heard = self.down.limit(sum, self.fs);
            let noise = self.noise * self.gaussian();
            out.push(heard + noise);
            self.down_next += step;
        }
        while self.down_first + (DOWN_REACH as f64) + 1.0 < self.down_next.floor() && self.down_levels.len() > 1 {
            self.down_levels.pop_front();
            self.down_first += 1.0;
        }
        out
    }

    /// The analogue modem's line samples since the last call in, and the
    /// level the digital modem gets out for this network sample.
    pub fn up(&mut self, samples: &[f64]) -> f64 {
        if self.up_delay > 0.0 {
            // The delay, as silence ahead of everything the modem says.
            let silence = (self.up_delay * self.fs).round() as usize;
            self.up_delay = 0.0;
            for _ in 0..silence {
                self.take_sample(0.0);
            }
        }
        let loop_noise = self.up_noise.unwrap_or(self.noise);
        for &x in samples {
            let limited = self.up.limit(x, self.fs);
            let noise = loop_noise * self.gaussian();
            self.take_sample(self.up_gain * limited + noise);
        }
        // The far end's jitter buffer, which on a loop moves where in our
        // waveform the A/D is reading rather than what it reads.
        self.up_now += 1;
        let periodic = self.up.slips.filter(|(every, _)| self.up_now.is_multiple_of(*every));
        let once = self.up.slip_at.filter(|(at, _)| self.up_now == *at);
        if let Some((_, inserted)) = periodic.or(once) {
            self.slip_up(inserted);
        }
        if let Some(inserted) = self.clock_slip() {
            self.slip_up(inserted);
        }
        let heard = match self.up_path {
            UpPath::Loop => self.sample_loop(),
            _ => self.to_codec.pop_front().unwrap_or(0.0),
        };
        // What the hybrid leaks of the downstream into the A/D's input, which
        // is what the digital modem's echo canceller is for (1 b)/V.92).
        let sum = match self.up.echo_of(&self.sent_down) {
            Some(echo) => heard + echo,
            None => heard,
        };
        let out = if self.quantised {
            self.carry_up(sum)
        } else {
            // No codeword is made, but `up_code` still has to be about this
            // sample rather than about the last one the quantiser saw.
            let (u, negative) = ucode::nearest(self.law, (sum * 32768.0).round() as i32);
            self.up_last = (u, !negative);
            sum
        };
        self.remember(Direction::Up, out);
        out
    }

    /// One line sample on its way to the codec: into the loop's waveform, or
    /// straight into the encoder's queue, or into the resampler chain that
    /// feeds it.
    fn take_sample(&mut self, x: f64) {
        self.line_count += 1;
        match self.up_path {
            UpPath::Loop => self.up_samples.push_back(x),
            UpPath::Straight { phase } => {
                self.prime_codec();
                let ratio = (self.fs / NETWORK_FS).round().max(1.0) as u64;
                if (self.line_count - 1) % ratio == phase as u64 % ratio {
                    self.up.buffer(x, self.slip_length, &mut self.to_codec);
                }
            }
            UpPath::Resampled { .. } => {
                self.prime_codec();
                if let Some(chain) = self.resampled.as_mut() {
                    chain.feed(x);
                }
                while let Some(v) = self.resampled.as_mut().and_then(|chain| chain.out.pop_front()) {
                    self.up.buffer(v, self.slip_length, &mut self.to_codec);
                }
            }
        }
    }

    /// The packet the far end's jitter buffer is holding before we say
    /// anything, once, at whatever a slip's length has been set to by then.
    ///
    /// A buffer that holds nothing has nothing to give up, and would answer a
    /// lost stretch with silence rather than with the material that follows
    /// it -- the same thing the loop path refuses to do. The standing packet
    /// costs the softphone paths a packet of delay, which is what a jitter
    /// buffer costs.
    fn prime_codec(&mut self) {
        if self.codec_primed {
            return;
        }
        self.codec_primed = true;
        self.to_codec.extend(std::iter::repeat_n(0.0, self.slip_length));
    }

    /// The A/D's sample off the loop: the anti-alias filter, at this network
    /// sample's own instant in the analogue modem's waveform.
    fn sample_loop(&mut self) -> f64 {
        let per = self.fs / ((1.0 + self.skew) * NETWORK_FS);
        let reach = UP_REACH * self.fs;
        // This network sample's instant, in the analogue modem's samples.
        let t = (self.up_next - UP_LAG - self.up_shift + self.up_phase).max(0.0) * per;
        self.up_next += 1.0;
        let centre = t.floor();
        self.tabulate_up_kernel(t - centre, reach);
        let base = centre as i64 - reach as i64;
        let mut sum = 0.0;
        for (k, weight) in self.up_kernel.iter().enumerate() {
            let index = (base + k as i64) as f64 - self.up_first;
            if index < 0.0 {
                continue;
            }
            let Some(&v) = self.up_samples.get(index as usize) else { continue };
            sum += v * weight;
        }
        // An inserted slip reads our waveform again, so the samples it reads
        // have to still be here.
        let keep = reach + 2.0 + self.buffered() as f64 * per;
        while self.up_first + keep < t && self.up_samples.len() > 1 {
            self.up_samples.pop_front();
            self.up_first += 1.0;
        }
        sum
    }

    /// Codewords the far end's buffer is holding, over and above the filter's
    /// own reach: none unless it slips, and a slip's worth if it does.
    fn buffered(&self) -> usize {
        if self.up.slips.is_some() || self.up.slip_at.is_some() { self.slip_length } else { 0 }
    }

    /// A slip the two clocks make on their own, and which way it goes.
    ///
    /// On a loop there is none: the far codec samples our waveform on the
    /// network's clock, which `with_clock` already is. On a softphone path our
    /// samples are forwarded as they are and the difference piles up in the
    /// far jitter buffer, which gives up a packet's worth every
    /// `slip_length / |skew|` codewords: 175 seconds at 114 ppm with the
    /// twenty-millisecond default. An analogue clock that runs fast overfills
    /// that buffer, so it throws a stretch away; a slow one starves it, so it
    /// makes one up.
    fn clock_slip(&self) -> Option<bool> {
        if self.up_path == UpPath::Loop || self.skew == 0.0 {
            return None;
        }
        let every = (self.slip_length as f64 / self.skew.abs()).round() as u64;
        (every > 0 && self.up_now.is_multiple_of(every)).then_some(self.skew > 0.0)
    }

    /// The far end's jitter buffer slipping, whichever kind of path it sits
    /// at the end of.
    fn slip_up(&mut self, inserted: bool) {
        match self.up_path {
            UpPath::Loop => self.slip_loop(inserted),
            _ => self.slip_codec(inserted),
        }
    }

    /// The far end's jitter buffer slipping on a softphone path, where what it
    /// holds is our codewords: it can make up a packet from the one it has
    /// just played out, which puts everything after it a packet later, or
    /// throw away the packet it is holding, which puts everything after it a
    /// packet earlier.
    ///
    /// What it holds is the standing packet plus the upstream leg's delay, and
    /// a buffer that has given all of that up has nothing more to lose, so
    /// that slip does not happen and `slips_up` does not count it. It is the
    /// same rule and the same slack the loop path keeps, where the leg's delay
    /// is what lets the A/D read ahead.
    fn slip_codec(&mut self, inserted: bool) {
        if inserted {
            let held = self.up.recent.len() as f64;
            for (k, v) in self.up.recent.iter().enumerate() {
                self.to_codec.push_back(v * (1.0 - k as f64 / held));
            }
            self.up.slip_count += 1;
            return;
        }
        if self.to_codec.len() < self.slip_length {
            return;
        }
        self.to_codec.drain(..self.slip_length);
        self.up.slip_count += 1;
    }

    /// The far end's jitter buffer slipping on a loop: made-up audio puts
    /// everything after it a slip's length later, and a lost stretch puts it
    /// that much earlier, because on a loop what it holds is our waveform and
    /// a slip is a move of where in it the A/D reads.
    ///
    /// Twenty milliseconds can only be thrown away by a buffer that is
    /// holding them, and here what it holds is the upstream leg's delay. A
    /// leg with nothing to give up cannot lose a stretch: the slip does not
    /// happen, and `slips_up` does not count it.
    fn slip_loop(&mut self, inserted: bool) {
        let per = self.fs / ((1.0 + self.skew) * NETWORK_FS);
        let reach = UP_REACH * self.fs;
        let length = self.slip_length as f64;
        if inserted {
            self.up_shift += length;
            self.up.slip_count += 1;
            return;
        }
        let ahead = (self.up_next - UP_LAG - (self.up_shift - length) + self.up_phase).max(0.0) * per;
        let newest = self.up_first + self.up_samples.len() as f64 - 1.0;
        if ahead + reach <= newest {
            self.up_shift -= length;
            self.up.slip_count += 1;
        }
    }

    /// The anti-alias filter's taps for a sampling instant `frac` of a line
    /// sample after a line sample, kept until that fraction moves.
    ///
    /// The taps are what the filter used to work out for every one of them on
    /// every network sample. With both clocks at the same rate the fraction
    /// never changes, so they are worked out once for a whole call: ten
    /// seconds of line costs 39 ms here against 268 ms per tap. When the
    /// clocks differ the sampling instant drifts and they are rebuilt every
    /// sample, which costs what the filter cost anyway -- 266 ms of the
    /// 305 ms a ten-second call 120 ppm off takes.
    ///
    /// Rounding the fraction to a grid would make it a table under a skew as
    /// well, and is refused. At 120 ppm the instant moves 2.4e-4 of a line
    /// sample per network sample, so a grid of 1/1024 would be reused about
    /// four times over -- and it moves 248 of 19 600 upstream codewords, one
    /// symbol in eighty of what a V.92 receiver decides on, with the coarser
    /// grids worse. A route that invents its own symbol errors cannot measure
    /// anyone else's, and four times over is not worth it.
    fn tabulate_up_kernel(&mut self, frac: f64, reach: f64) {
        let taps = reach as i64;
        let wanted = 2 * taps as usize + 1;
        if self.up_kernel_at == Some(frac.to_bits()) && self.up_kernel.len() == wanted {
            return;
        }
        let cutoff = self.up_cutoff / self.fs;
        self.up_kernel.clear();
        self.up_kernel.reserve(wanted);
        for d in -taps..=taps {
            self.up_kernel.push(kernel(frac - d as f64, cutoff, reach + 1.0));
        }
        self.up_kernel_at = Some(frac.to_bits());
    }
}

/// The nearest codeword of `law` to a level, as a level again.
fn quantise(law: Law, level: f64) -> f64 {
    let (u, negative) = ucode::nearest(law, (level * 32768.0).round() as i32);
    ucode::level(law, u) * if negative { -1.0 } else { 1.0 }
}

/// A transcoding gateway's low-pass, as the taps of a symmetric filter at the
/// network's own rate.
///
/// The response wanted is a raised cosine in amplitude: flat up to `start`,
/// nothing from `end`, and half of one plus a cosine in between. The taps are
/// that response sampled at the `TRANSCODER_GRID` points of the band and
/// transformed back, with the two end taps halved because they are the one tap
/// of the design's own length split between them -- so the filter's gain is
/// exactly the wanted response at every one of those frequencies, and within
/// 0.2 dB of it between them. Its gain at DC is one, and it costs half the
/// design's length in delay.
fn low_pass_taps(start: f64, end: f64) -> Vec<f64> {
    let n = TRANSCODER_GRID as f64;
    let half = TRANSCODER_GRID as i64 / 2;
    let amplitude = |f: f64| {
        if f <= start {
            1.0
        } else if f >= end {
            0.0
        } else {
            0.5 * (1.0 + (PI * (f - start) / (end - start)).cos())
        }
    };
    let mut taps = Vec::with_capacity(2 * half as usize + 1);
    for k in -half..=half {
        let mut sum = amplitude(0.0);
        for m in 1..half {
            let f = m as f64 * NETWORK_FS / n;
            sum += 2.0 * amplitude(f) * (2.0 * PI * m as f64 * k as f64 / n).cos();
        }
        // The band edge's own point, whose cosine is exactly one or minus one.
        sum += amplitude(NETWORK_FS / 2.0) * if k % 2 == 0 { 1.0 } else { -1.0 };
        taps.push(sum / n);
    }
    taps[0] *= 0.5;
    let last = taps.len() - 1;
    taps[last] *= 0.5;
    taps
}

/// A windowed sinc at `cutoff` cycles a sample, reaching `edge` samples,
/// with unit gain at DC.
fn kernel(t: f64, cutoff: f64, edge: f64) -> f64 {
    if t.abs() >= edge {
        return 0.0;
    }
    let x = 2.0 * cutoff * t;
    let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
    let window = 0.42 + 0.5 * (std::f64::consts::PI * t / edge).cos() + 0.08 * (2.0 * std::f64::consts::PI * t / edge).cos();
    2.0 * cutoff * sinc * window
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 16_000.0;

    #[test]
    fn a_codeword_goes_down_and_comes_back_up_as_itself() {
        let mut net = Network::new(Law::Mu, 16_000.0).with_upstream_gain(1.0);
        let level = ucode::level(Law::Mu, 90);
        // A steady level: the filters settle to it both ways.
        let mut up = 0.0;
        for _ in 0..400 {
            let heard = net.down(level);
            up = net.up(&heard);
        }
        assert!((up - level).abs() < 0.01 * level, "{up} against {level}");
    }

    #[test]
    fn a_fast_clock_hears_more_samples() {
        let mut net = Network::new(Law::Mu, 16_000.0).with_clock(500.0);
        let mut heard = 0usize;
        for _ in 0..80_000 {
            heard += net.down(0.1).len();
        }
        // Ten seconds of network at 16 kHz is 160 000 samples; half a
        // thousandth fast is 80 more.
        assert!((heard as i64 - 160_080).abs() < 60, "{heard}");
    }

    #[test]
    fn a_slip_moves_everything_after_it_by_160_codewords() {
        for inserted in [true, false] {
            let mut net = Network::new(Law::Mu, 16_000.0).with_slips(1.0, inserted);
            let mut heard = 0usize;
            for _ in 0..12_000 {
                heard += net.down(0.1).len();
            }
            assert_eq!(net.slips(), 1);
            let expected = 24_000i64 + if inserted { 320 } else { -320 };
            assert!((heard as i64 - expected).abs() < 60, "{inserted}: {heard}");
        }
    }

    #[test]
    fn a_robbed_bit_moves_every_other_codeword_in_one_octet_of_six() {
        let mut net = Network::new(Law::Mu, 16_000.0).with_robbed_bit(3);
        let mut moved = [0usize; 6];
        for n in 0..600 {
            let u = (n % 128) as u8;
            let level = ucode::level(Law::Mu, u);
            let carried = net.carry(level);
            if (carried - level).abs() > 1e-9 {
                moved[n % 6] += 1;
            }
        }
        assert_eq!(moved[0], 0);
        assert!(moved[3] > 40, "{moved:?}");
        assert_eq!(moved.iter().sum::<usize>(), moved[3]);
    }

    /// Four numbers from a counter, so the same samples go through both the
    /// tabulated filter and the one it replaced.
    fn random(seed: &mut u64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    }

    /// The upstream A/D as it was written before its kernel was tabulated:
    /// every tap worked out from sines and cosines, for every sample.
    ///
    /// The lag, the cutoff and the way a clock `ppm` off and a sampling phase
    /// move the instant are spelled out here rather than taken from the
    /// network, so that a mis-transcribed one of them is a failure and not an
    /// agreement.
    fn kernel_per_tap(history: &[f64], up_next: f64, fs: f64, ppm: f64, phase: f64) -> f64 {
        let per = fs / ((1.0 - ppm * 1e-6) * NETWORK_FS);
        let reach = UP_REACH * fs;
        let lag = DOWN_REACH as f64 + 2.0 + UP_REACH * NETWORK_FS;
        let t = (up_next - lag + phase).max(0.0) * per;
        let centre = t.floor() as i64;
        let cutoff = 3700.0 / fs;
        let mut sum = 0.0;
        for j in centre - reach as i64..=centre + reach as i64 {
            if j < 0 {
                continue;
            }
            let Some(&v) = history.get(j as usize) else { continue };
            sum += v * kernel(t - j as f64, cutoff, reach + 1.0);
        }
        sum
    }

    /// 6.2: the A/D samples on the network's clock, so with the two clocks
    /// the same its instants repeat exactly and one table of taps serves the
    /// whole call. It has to be the filter that was there before it, to the
    /// last bit, or every V.90 test is a different test.
    ///
    /// 8.6.3's phase and a sound card's clock both move the instant off a
    /// line sample, and neither may move a level, so the table is checked
    /// where it is kept and where it is rebuilt every sample. That is also
    /// what stands in the way of rounding the phase to a grid to make the
    /// skewed case a table again: the grid would be a level change, and this
    /// says so.
    #[test]
    fn the_tabulated_upstream_kernel_gives_the_same_levels_as_before() {
        // Not a phase of a half: with no skew that lands the fraction on the
        // window's own symmetry, where a tap read from the wrong side of the
        // table would still match.
        for (ppm, phase) in [(0.0, 0.0), (0.0, 0.3), (120.0, 0.0), (-120.0, 0.37)] {
            let mut net = Network::new(Law::Mu, FS)
                .with_upstream_gain(1.0)
                .unquantised()
                .with_clock(ppm)
                .with_upstream_phase(phase);
            let mut history: Vec<f64> = Vec::new();
            let mut seed = 0x1234_5678_9abc_def1u64;
            for n in 0..2000u64 {
                let pair = [random(&mut seed), random(&mut seed)];
                history.extend_from_slice(&pair);
                let got = net.up(&pair);
                let want = kernel_per_tap(&history, n as f64, FS, ppm, phase);
                assert_eq!(
                    got.to_bits(),
                    want.to_bits(),
                    "{ppm} ppm, phase {phase}, sample {n}: {got} against {want}"
                );
            }
        }
    }

    /// 8.6.3: the digital modem cannot move the central office A/D's sampling
    /// phase, so the model has to be able to put it anywhere within a symbol.
    /// A windowed sinc passes a ramp through as its value at the sampling
    /// instant, so a quarter of a symbol at 16 kHz is half a line sample.
    #[test]
    fn a_fractional_upstream_phase_samples_between_our_samples() {
        const SLOPE: f64 = 1e-5;
        let at = |phase: f64| {
            let mut net =
                Network::new(Law::Mu, FS).with_upstream_gain(1.0).unquantised().with_upstream_phase(phase);
            let mut out = 0.0;
            for n in 0..1200u64 {
                let k = 2 * n;
                out = net.up(&[k as f64 * SLOPE, (k + 1) as f64 * SLOPE]);
            }
            out
        };
        let base = at(0.0);
        for phase in [0.25, 0.5, 0.75] {
            // Every network sample is two line samples at 16 kHz.
            let want = base + phase * 2.0 * SLOPE;
            let got = at(phase);
            assert!((got - want).abs() < 1e-4 * SLOPE, "phase {phase}: {got} against {want}");
        }
    }

    /// With the A/D on our own sampling instants and nothing on the loop, the
    /// codeword the digital modem is handed is the one the analogue modem
    /// meant to send. A V.92 receiver decides on these, not on levels, and
    /// A-law's Ucode 0 is not silence, so the sign has to come back too.
    ///
    /// Taking the quantiser out is how a test asks what quantising costs, and
    /// it must not leave this answering about a sample from before that.
    #[test]
    fn the_upstream_codeword_is_the_level_we_meant() {
        for law in [Law::Mu, Law::A] {
            for quantised in [true, false] {
                let mut net = Network::new(law, FS).with_upstream_gain(1.0);
                if !quantised {
                    net = net.unquantised();
                }
                for u in [0u8, 1, 7, 16, 45, 90, 127] {
                    for positive in [true, false] {
                        let level = ucode::level(law, u) * if positive { 1.0 } else { -1.0 };
                        if level == 0.0 && !positive {
                            // mu-law's zero has no sign to carry.
                            continue;
                        }
                        for _ in 0..250 {
                            net.up(&[level, level]);
                        }
                        assert_eq!(
                            net.up_code(),
                            (u, positive),
                            "{law:?} Ucode {u}, positive {positive}, quantised {quantised}"
                        );
                    }
                }
            }
        }
    }

    /// A T1 robs bits both ways round, and the upstream octets are counted
    /// from the first codeword the A/D made: the two phases are unrelated.
    #[test]
    fn an_upstream_robbed_bit_moves_codewords_only_in_its_own_octet_of_six() {
        // A mu-law octet of an even Ucode has its bottom bit set already, so
        // both phases here are odd ones, where the robbing shows.
        let mut net = Network::new(Law::Mu, FS).with_robbed_bit(1).with_upstream_robbed_bit(3);
        let mut moved = [0usize; 6];
        let mut moved_down = [0usize; 6];
        for n in 0..600 {
            let u = (n % 128) as u8;
            let level = ucode::level(Law::Mu, u);
            let carried = net.carry_up(level);
            if (carried - level).abs() > 1e-9 {
                moved[n % 6] += 1;
            }
            if (net.carry(level) - level).abs() > 1e-9 {
                moved_down[n % 6] += 1;
            }
        }
        assert_eq!(moved[0], 0);
        assert!(moved[3] > 40, "{moved:?}");
        assert_eq!(moved.iter().sum::<usize>(), moved[3]);
        // And the downstream kept its own phase.
        assert!(moved_down[1] > 40, "{moved_down:?}");
        assert_eq!(moved_down.iter().sum::<usize>(), moved_down[1]);

        // Through the public path as well, or nothing says the A/D reaches
        // the robbing at all. A steady odd Ucode's mu-law octet has its
        // bottom bit clear, so the robbed octet of six comes back one Ucode
        // lower and every other one comes back as itself.
        let mut net = Network::new(Law::Mu, FS).with_upstream_gain(1.0).with_upstream_robbed_bit(3);
        let level = ucode::level(Law::Mu, 45);
        for n in 0..600usize {
            net.up(&[level, level]);
            if n >= 200 {
                // The upstream octets are counted from the A/D's first
                // codeword, so the phase is the call number.
                let want = if n % 6 == 3 { 44 } else { 45 };
                assert_eq!(net.up_code(), (want, true), "call {n}");
            }
        }
    }

    /// A pad in the network is a digital one: it scales the codeword and the
    /// result is a codeword again. The downstream's pad leaves the upstream
    /// alone.
    #[test]
    fn an_upstream_pad_scales_every_level() {
        let mut net = Network::new(Law::Mu, FS).with_pads(3.0, 6.0);
        for u in [8u8, 30, 60, 90, 120] {
            for sign in [1.0, -1.0] {
                let level = ucode::level(Law::Mu, u) * sign;
                let padded = net.carry_up(level);
                let want = level * 10f64.powf(-6.0 / 20.0);
                assert_eq!(padded.to_bits(), net.quantise(want).to_bits(), "Ucode {u}");
                assert!((padded - level / 2.0).abs() < 0.07 * level.abs(), "Ucode {u}: {padded}");
            }
        }
        let mut clean = Network::new(Law::Mu, FS).with_pads(6.0, 0.0);
        let level = ucode::level(Law::Mu, 90);
        assert_eq!(clean.carry_up(level).to_bits(), level.to_bits());

        // Through the public path as well, or nothing says the A/D reaches
        // the pad at all.
        let mut net = Network::new(Law::Mu, FS).with_upstream_gain(1.0).with_pads(0.0, 6.0);
        let mut out = 0.0;
        for _ in 0..400 {
            out = net.up(&[level, level]);
        }
        let halved = net.quantise(level * 10f64.powf(-6.0 / 20.0));
        assert_eq!(out.to_bits(), halved.to_bits(), "{out} against {halved}");
        assert_eq!(ucode::level(Law::Mu, net.up_code().0).to_bits(), halved.to_bits());
        assert!(net.up_code().1, "the level sent was positive");
    }

    /// The far end's buffer slips our upstream as ours slips the downstream:
    /// made-up audio puts everything after it a slip later, a lost stretch
    /// puts it a slip earlier. A ramp says which of our samples arrived.
    #[test]
    fn an_upstream_slip_moves_everything_after_it_by_the_slip_length() {
        const SLOPE: f64 = 1e-5;
        let heard_at = |length: usize, slip: Option<bool>| {
            let mut net = Network::new(Law::Mu, FS)
                .with_delays(0.0, 0.1)
                .with_upstream_gain(1.0)
                .unquantised()
                .with_slip_length(length);
            if let Some(inserted) = slip {
                net = net.with_slip_at_in(Direction::Up, 0.05, inserted);
            }
            let mut out = 0.0;
            for n in 0..1200u64 {
                let k = 2 * n;
                out = net.up(&[k as f64 * SLOPE, (k + 1) as f64 * SLOPE]);
            }
            (out, net.slips_up(), net.slips())
        };
        for length in [SLIP, 80] {
            let (clean, none, _) = heard_at(length, None);
            assert_eq!(none, 0);
            for inserted in [true, false] {
                let (slipped, count, down) = heard_at(length, Some(inserted));
                assert_eq!(count, 1, "{length} codewords, inserted {inserted}");
                assert_eq!(down, 0, "an upstream slip is not a downstream one");
                // Two line samples to the codeword, and made-up audio puts us
                // back in the ramp while a lost stretch puts us forward.
                let moved = (slipped - clean) / (2.0 * SLOPE);
                let want = if inserted { -(length as f64) } else { length as f64 };
                assert!((moved - want).abs() < 0.01, "{length} codewords, inserted {inserted}: {moved}");
            }
        }
    }

    /// The cuts heard on live calls are ten milliseconds, not twenty, and
    /// eighty codewords move a twelve-symbol upstream frame by eight symbols
    /// where 160 move it by four.
    #[test]
    fn a_ten_millisecond_cut_moves_everything_by_80_codewords() {
        let mut net = Network::new(Law::Mu, FS).with_slip_length(80).with_slips(1.0, false);
        let mut heard = 0usize;
        for _ in 0..12_000 {
            heard += net.down(0.1).len();
        }
        assert_eq!(net.slips_down(), 1);
        // Twelve thousand codewords is 24 000 line samples; eighty codewords
        // cut is 160 fewer.
        assert!((heard as i64 - (24_000 - 160)).abs() < 60, "{heard}");
    }

    /// The two legs of a call need not take the same route, and the
    /// acknowledgement windows V.92 counts in round-trip delays care which
    /// leg the time is on.
    #[test]
    fn unequal_legs_delay_each_direction_by_its_own_amount() {
        let legs = |down: f64, up: f64| {
            let mut net = Network::new(Law::Mu, FS).with_delays(down, up).with_upstream_gain(1.0).unquantised();
            let mut silence = None;
            let mut arrived = None;
            for n in 0..4000u64 {
                let said = if n >= 100 { [0.5, 0.5] } else { [0.0, 0.0] };
                let out = net.up(&said);
                if arrived.is_none() && out.abs() > 1e-6 {
                    arrived = Some(n);
                }
                let heard = net.down(0.0);
                if silence.is_none() {
                    silence = Some(heard.len());
                }
            }
            (silence.unwrap(), arrived.unwrap())
        };
        let (short_leg, late) = legs(0.05, 0.2);
        let (long_leg, early) = legs(0.2, 0.05);
        // The downstream delay comes out as silence before anything else.
        assert_eq!(short_leg, 800);
        assert_eq!(long_leg, 3200);
        // And 150 ms more upstream is 1200 codewords later.
        assert_eq!(late - early, 1200, "{late} against {early}");
    }

    /// 1 b)/V.92: "channel separation by echo cancellation techniques". The
    /// hybrid leaks each direction into the other, and a canceller can only be
    /// tested against a route that does it -- at the level and the delay the
    /// taps were given, and both ways round from the one builder.
    ///
    /// The upstream is measured with an impulse, because there the echo is
    /// added to the A/D's own sample and nothing spreads it. The downstream is
    /// measured with a steady level, because there it goes through the codec's
    /// reconstruction, which spreads an impulse over forty codewords but
    /// passes a steady level at the gain it was given.
    #[test]
    fn the_hybrid_echo_is_where_it_was_put() {
        const DELAY: usize = 3;
        let taps = [0.0, 0.0, 0.0, 1.0];
        let gain = 10f64.powf(-40.0 / 20.0);
        let level = ucode::level(Law::Mu, 100);

        // Downstream into upstream: one codeword sent, silence after it.
        let mut net = Network::new(Law::Mu, FS).with_upstream_gain(1.0).unquantised().with_echo(40.0, &taps);
        let mut heard = Vec::new();
        for n in 0..40 {
            net.down(if n == 0 { level } else { 0.0 });
            heard.push(net.up(&[0.0, 0.0]));
        }
        let want = gain * level;
        assert!((heard[DELAY] - want).abs() < 1e-12 * want, "{} against {want}", heard[DELAY]);
        for (n, &x) in heard.iter().enumerate() {
            if n != DELAY {
                assert!(x.abs() < 1e-3 * want, "codeword {n}: {x}");
            }
        }

        // Upstream into downstream: a steady level said, and nothing sent.
        let mut net = Network::new(Law::Mu, FS).with_upstream_gain(1.0).unquantised().with_echo(40.0, &taps);
        let mut back = 0.0;
        for _ in 0..400 {
            net.up(&[level, level]);
            for x in net.down(0.0) {
                back = x;
            }
        }
        assert!((back - want).abs() < 1e-3 * want, "{back} against {want}");
    }

    /// A live V.34 far end echoed us back only 25 dB down, a round trip late.
    /// That is one tap a long way back rather than a filter, and it is not the
    /// hybrid's: asking for it downstream must leave the upstream clean.
    #[test]
    fn a_far_echo_comes_back_a_round_trip_later() {
        const DELAY: usize = 400;
        let gain = 10f64.powf(-25.0 / 20.0);
        let level = ucode::level(Law::Mu, 100);
        let heard = |net: Network| {
            let mut net = net;
            let mut heard = Vec::new();
            for n in 0..500 {
                net.down(if n == 0 { level } else { 0.0 });
                heard.push(net.up(&[0.0, 0.0]));
            }
            heard
        };
        let clean = Network::new(Law::Mu, FS).with_upstream_gain(1.0).unquantised();
        let far = heard(clean.clone().with_far_echo_in(Direction::Up, 25.0, DELAY as f64 / NETWORK_FS));
        let want = gain * level;
        assert!((far[DELAY] - want).abs() < 1e-12 * want, "{} against {want}", far[DELAY]);
        for (n, &x) in far.iter().enumerate() {
            if n != DELAY {
                assert!(x.abs() < 1e-3 * want, "codeword {n}: {x}");
            }
        }
        // The analogue side's own far echo is the default, and it is not this.
        let analogue = heard(clean.with_far_echo(25.0, DELAY as f64 / NETWORK_FS));
        assert!(analogue.iter().all(|x| x.abs() < 1e-12), "a downstream far echo reached the upstream");
    }

    /// The Crazytel path decodes mu-law, low-passes and re-encodes as A-law,
    /// and its low-pass was measured 3 dB down at 3750 Hz and 18 dB down at
    /// 4000 Hz. Those two points are what the model has to reproduce; the
    /// shape between them is a guess, so what is asserted there is only that
    /// the passband is flat and that the filter is really in the leg.
    #[test]
    fn the_transcoder_is_3_db_down_at_3750_and_18_db_down_at_4000() {
        let taps = low_pass_taps(CRAZYTEL_LOW_PASS.0, CRAZYTEL_LOW_PASS.1);
        let middle = (taps.len() - 1) as i64 / 2;
        let db = |hz: f64| {
            let gain: f64 = taps
                .iter()
                .enumerate()
                .map(|(i, t)| t * (2.0 * PI * hz * (i as i64 - middle) as f64 / NETWORK_FS).cos())
                .sum();
            20.0 * gain.abs().log10()
        };
        assert!((db(3750.0) + 3.0).abs() < 0.01, "3750 Hz: {} dB", db(3750.0));
        assert!((db(4000.0) + 18.0).abs() < 0.01, "4000 Hz: {} dB", db(4000.0));
        for hz in (0..=3400).step_by(100) {
            assert!(db(hz as f64).abs() < 0.25, "{hz} Hz: {} dB", db(hz as f64));
        }

        // And it is in the leg: a steady level comes through a gateway
        // unchanged, and the alternation that is 4 kHz at 8000 codewords a
        // second comes through 18 dB down.
        let mut net = Network::new(Law::Mu, FS).with_transcoder(Law::Mu, Some(CRAZYTEL_LOW_PASS));
        let level = ucode::level(Law::Mu, 110);
        let mut steady = 0.0;
        for _ in 0..200 {
            steady = net.carry(level);
        }
        assert_eq!(steady.to_bits(), net.quantise(level).to_bits(), "a steady level lost something");
        let mut edge = 0.0;
        for n in 0..200 {
            edge = net.carry(if n % 2 == 0 { level } else { -level });
        }
        let want = level * 10f64.powf(-18.0 / 20.0);
        assert!((edge.abs() - want).abs() < 0.05 * want, "{} against {want}", edge.abs());
    }

    /// The gateway re-encodes on the other law's grid, so a leg through one
    /// cannot carry the codeword the modem meant even with no filter at all:
    /// that is the signal-dependent noise the Crazytel path adds, and it lands
    /// after any equalising the prefilter could have done.
    #[test]
    fn a_transcoded_leg_re_encodes_in_the_gateways_law() {
        let mut net = Network::new(Law::Mu, FS)
            .with_upstream_gain(1.0)
            .with_transcoder_in(Direction::Up, Law::A, None);
        let mut moved = 0;
        for u in 20..120u8 {
            let level = ucode::level(Law::Mu, u);
            let mut out = 0.0;
            for _ in 0..400 {
                out = net.up(&[level, level]);
            }
            // Whatever comes out is a level of the gateway's law.
            assert_eq!(out.to_bits(), quantise(Law::A, out).to_bits(), "Ucode {u} left the A-law grid");
            if (out - level).abs() > 1e-12 {
                moved += 1;
            }
        }
        assert!(moved > 80, "only {moved} of 100 codewords moved");

        // The downstream leg was not asked for a gateway and has none.
        let level = ucode::level(Law::Mu, 90);
        assert_eq!(net.carry(level).to_bits(), level.to_bits());
    }

    /// With both ends of the call ours over VoIP there is no analogue loop:
    /// the softphone hands one sample in two to its G.711 encoder and the
    /// packets carry it verbatim. Only one of the two phases carries the
    /// codewords; the other carries whatever was put between them. The far
    /// end's buffer holds a packet, so what arrives is what was written a
    /// packet earlier -- and it is written exactly.
    #[test]
    fn a_straight_softphone_path_hands_the_encoder_our_samples_exactly() {
        let ucode_at = |k: usize| 20 + (k % 100) as u8;
        for phase in [0, 1] {
            let mut net =
                Network::new(Law::Mu, FS).with_upstream_gain(1.0).with_up_path(UpPath::Straight { phase });
            let mut heard = Vec::new();
            for k in 0..SLIP + 200 {
                let out = net.up(&[ucode::level(Law::Mu, ucode_at(k)), 0.0]);
                heard.push((net.up_code(), out));
            }
            for (k, &(code, out)) in heard.iter().enumerate().skip(SLIP) {
                let u = ucode_at(k - SLIP);
                let level = if phase == 0 { ucode::level(Law::Mu, u) } else { 0.0 };
                let want = if phase == 0 { (u, true) } else { (0, true) };
                assert_eq!(code, want, "phase {phase}, codeword {k}");
                assert_eq!(out.to_bits(), level.to_bits(), "phase {phase}, codeword {k}");
            }
        }
    }

    /// The other thing a softphone may do is resample -- up to 48 kHz, through
    /// its buffer, and down to 8 kHz -- and nothing survives that as a
    /// codeword, because the kernel that comes back down is 6 dB down at the
    /// new Nyquist and the codewords sit right against it.
    #[test]
    fn a_resampled_path_does_not() {
        let mut net =
            Network::new(Law::Mu, FS).with_upstream_gain(1.0).with_up_path(UpPath::Resampled { delay: 0 });
        let mut exact = 0;
        let mut sent = 0.0;
        let mut got = 0.0;
        for k in 0..SLIP + 400 {
            let u = 20 + (k % 100) as u8;
            let level = ucode::level(Law::Mu, u);
            let out = net.up(&[level, 0.0]);
            if k >= SLIP + 100 {
                if net.up_code() == (u, true) {
                    exact += 1;
                }
                sent += level * level;
                got += out * out;
            }
        }
        assert!(exact < 15, "{exact} of 300 codewords came back as themselves");
        assert!(got < 0.5 * sent, "the alternation came through: {got} against {sent}");
    }

    /// 6.2: the upstream symbol rate is the network's. On a loop the far codec
    /// samples our waveform on that clock, which is a resampling. On a
    /// softphone path our samples are forwarded as they are and the difference
    /// piles up in the far jitter buffer instead, which gives up a packet
    /// every `slip_length / |offset|` codewords -- twenty milliseconds every
    /// 175 seconds at 114 ppm.
    #[test]
    fn a_clock_off_on_a_softphone_path_becomes_slips() {
        // A tenth of a second of upstream leg, so the far buffer has more than
        // its standing packet to give up: a route with only the one packet in
        // it can lose a stretch once, and then has nothing left, exactly as a
        // loop with no delay cannot read ahead.
        let slips = |ppm: f64, seconds: f64, path: UpPath| {
            let mut net = Network::new(Law::Mu, FS).with_delays(0.0, 0.1).with_clock(ppm).with_up_path(path);
            for _ in 0..(seconds * NETWORK_FS) as usize {
                net.up(&[0.0, 0.0]);
            }
            net.slips_up()
        };
        let straight = UpPath::Straight { phase: 0 };
        // 160 codewords / 114e-6 is 1 403 509 of them: 175.4 seconds.
        assert_eq!(slips(114.0, 175.0, straight), 0);
        assert_eq!(slips(114.0, 176.0, straight), 1);
        // Ten times the offset is a tenth of the interval: 17.54 seconds.
        assert_eq!(slips(1140.0, 60.0, straight), 3);
        // A loop resamples instead, so it never slips on the clock alone.
        assert_eq!(slips(1140.0, 20.0, UpPath::Loop), 0);

        // Which way it slips: a fast clock overfills the far buffer, so a
        // stretch is thrown away and what arrives jumps forward; a slow one
        // starves it, so a stretch is made up and what arrives falls back.
        const SLOPE: f64 = 1e-6;
        let ramp = |ppm: f64| {
            let mut net = Network::new(Law::Mu, FS)
                .with_delays(0.0, 0.1)
                .with_clock(ppm)
                .unquantised()
                .with_upstream_gain(1.0)
                .with_up_path(UpPath::Straight { phase: 0 });
            let mut out = 0.0;
            for n in 0..24_000u64 {
                let k = 2 * n;
                out = net.up(&[k as f64 * SLOPE, (k + 1) as f64 * SLOPE]);
            }
            (out, net.slips_up())
        };
        let (clean, none) = ramp(0.0);
        assert_eq!(none, 0);
        for (ppm, want) in [(11_400.0, 320.0), (-11_400.0, -320.0)] {
            let (slipped, count) = ramp(ppm);
            assert_eq!(count, 1, "{ppm} ppm");
            let moved = (slipped - clean) / SLOPE;
            assert!((moved - want).abs() < 1.0, "{ppm} ppm: {moved} line samples, wanted {want}");
        }
    }

    /// A softphone's capture path may limit what it sends as its playback
    /// limits what it plays, and precoding raises the peak-to-average ratio,
    /// so the prefilter's output can go over a ceiling the constellation never
    /// reaches. What that costs is not only the loud sample: the gain stays
    /// down and reads the ones after it low, for as long as the release.
    #[test]
    fn an_upstream_gain_control_squashes_loud_samples_and_the_ones_after() {
        let run = |controlled: bool| {
            let mut net = Network::new(Law::Mu, FS).with_upstream_gain(1.0).unquantised();
            if controlled {
                net = net.with_upstream_gain_control(0.8, 0.05);
            }
            let mut heard = Vec::new();
            for n in 0..4000 {
                let x = if (400..800).contains(&n) { 2.0 } else { 0.5 };
                heard.push(net.up(&[x, x]));
            }
            heard
        };
        // The A/D reads 86 codewords behind, so the loud stretch arrives at
        // codewords 486 to 886.
        let free = run(false);
        assert!((free[800] - 2.0).abs() < 0.01, "unlimited: {}", free[800]);
        let held = run(true);
        assert!((held[480] - 0.5).abs() < 0.01, "before: {}", held[480]);
        assert!((held[800] - 0.8).abs() < 0.01, "squashed: {}", held[800]);
        // A quarter of the gain at the moment the loud stretch ends, back
        // within a hundredth of where it was five releases later.
        assert!(held[990] > 0.2 && held[990] < 0.35, "just after: {}", held[990]);
        assert!((held[3000] - 0.5).abs() < 0.01, "recovered: {}", held[3000]);
    }

    /// Upstream and downstream margins are different questions, so the two
    /// directions carry their own noise.
    #[test]
    fn each_direction_carries_its_own_noise() {
        let mut net =
            Network::new(Law::Mu, FS).with_noise(0.0).with_upstream_noise(1e-2).with_upstream_gain(1.0).unquantised();
        let mut power = 0.0;
        let mut loudest = 0f64;
        for _ in 0..2000 {
            let out = net.up(&[0.0, 0.0]);
            power += out * out;
            for x in net.down(0.0) {
                loudest = loudest.max(x.abs());
            }
        }
        assert_eq!(loudest, 0.0, "the downstream was asked for no noise");
        // The anti-alias filter has unit gain at DC, but noise is not DC:
        // what gets through is the sum of the squares of its taps, which for
        // 3700 Hz at 16 kHz is 0.459 -- a little under half the power, and
        // 0.68 of the level. Worked out here rather than written down, so
        // that a different cutoff moves the expectation with it.
        let reach = UP_REACH * FS;
        let power_gain: f64 =
            (-(reach as i64)..=reach as i64).map(|d| kernel(d as f64, UP_CUTOFF / FS, reach + 1.0).powi(2)).sum();
        let want = 1e-2 * power_gain.sqrt();
        let rms = (power / 2000.0).sqrt();
        // A tenth either way: the samples at the start have only part of a
        // window to fill it, and 2000 of them is not an infinity of them.
        assert!((rms - want).abs() < 0.1 * want, "{rms} against {want}");
    }
}
