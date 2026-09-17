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
//! which, the clocks being the same, it usually never does.
//!
//! Nothing here is a claim about any real network, only about what V.90 and
//! V.92 have to get through.

use std::collections::VecDeque;

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

/// Which way round a leg of the route runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Digital modem to analogue modem.
    Down,
    /// Analogue modem to digital modem.
    Up,
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
    /// Which of the six octets a robbed bit lands on, if one does, each way
    /// round with its own phase.
    robbed: Option<usize>,
    octets: usize,
    up_robbed: Option<usize>,
    up_octets: usize,
    /// A digital pad, as a gain on every level, each way round.
    pad: f64,
    up_pad: f64,
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
    /// Slips: how often, and whether audio is made up or lost; or one, at a
    /// given codeword; each way round, and how long a slip is.
    slips: Option<(u64, bool)>,
    slip_at: Option<(u64, bool)>,
    up_slips: Option<(u64, bool)>,
    up_slip_at: Option<(u64, bool)>,
    slip_length: usize,
    /// Codewords of a lost stretch still to drop.
    dropping: usize,
    /// The last slip's worth of codewords, for concealment to repeat.
    recent: VecDeque<f64>,
    slip_count: u32,
    up_slip_count: u32,
    /// How far the far end's buffer has moved our upstream, in codewords:
    /// positive is later.
    up_shift: f64,
    /// A softphone's gain control on what it plays: the loudest it lets
    /// through, how fast it recovers, in seconds, and where it has got to.
    gain_control: Option<(f64, f64)>,
    gain: f64,
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
            robbed: None,
            octets: 0,
            up_robbed: None,
            up_octets: 0,
            pad: 1.0,
            up_pad: 1.0,
            quantised: true,
            up_gain: 0.25,
            up_phase: 0.0,
            up_cutoff: UP_CUTOFF,
            up_last: (0, true),
            up_kernel: Vec::new(),
            up_kernel_at: None,
            slips: None,
            slip_at: None,
            up_slips: None,
            up_slip_at: None,
            slip_length: SLIP,
            dropping: 0,
            recent: VecDeque::with_capacity(SLIP),
            slip_count: 0,
            up_slip_count: 0,
            up_shift: 0.0,
            gain_control: None,
            gain: 1.0,
        }
    }

    /// Each way's delay, in seconds of line.
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
        self.robbed = Some(phase % 6);
        self
    }

    /// A robbed bit on every sixth upstream octet, starting at `phase`.
    ///
    /// A T1 robs bits in both directions, and the two phases have nothing to
    /// do with each other: the upstream octets are counted from the first
    /// codeword the A/D made, not from the first one sent down.
    pub fn with_upstream_robbed_bit(mut self, phase: usize) -> Self {
        self.up_robbed = Some(phase % 6);
        self
    }

    /// A digital pad of `db` on the downstream.
    pub fn with_pad(self, db: f64) -> Self {
        self.with_pads(db, 0.0)
    }

    /// A digital pad on each direction, in decibels: a scale applied to the
    /// codeword the route is carrying, and then requantised, since a pad in
    /// the network is a digital one and its output is a codeword again.
    pub fn with_pads(mut self, down_db: f64, up_db: f64) -> Self {
        self.pad = 10f64.powf(-down_db / 20.0);
        self.up_pad = 10f64.powf(-up_db / 20.0);
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
    /// the Crazytel path measured -3 dB at 3.75 kHz and -18 dB at 4 kHz, and
    /// the upstream constellation lives right against that edge.
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

    /// A downstream slip every `seconds`: twenty milliseconds made up if
    /// `inserted`, lost otherwise.
    pub fn with_slips(self, seconds: f64, inserted: bool) -> Self {
        self.with_slips_in(Direction::Down, seconds, inserted)
    }

    /// A slip in one direction every `seconds`: a slip's worth of audio made
    /// up if `inserted`, lost otherwise.
    pub fn with_slips_in(mut self, dir: Direction, seconds: f64, inserted: bool) -> Self {
        let every = Some(((seconds * NETWORK_FS) as u64, inserted));
        match dir {
            Direction::Down => self.slips = every,
            Direction::Up => self.up_slips = every,
        }
        self
    }

    /// A gain control on the downstream as the analogue modem hears it:
    /// anything louder than `ceiling` of full scale is turned down to it at
    /// once, and the gain comes back up over `release` seconds -- what a live
    /// call through a softphone did to codewords above about a third of full
    /// scale.
    pub fn with_gain_control(mut self, ceiling: f64, release: f64) -> Self {
        self.gain_control = Some((ceiling, release));
        self
    }

    /// One downstream slip, `seconds` into the call.
    pub fn with_slip_at(self, seconds: f64, inserted: bool) -> Self {
        self.with_slip_at_in(Direction::Down, seconds, inserted)
    }

    /// One slip in one direction, `seconds` into the call.
    pub fn with_slip_at_in(mut self, dir: Direction, seconds: f64, inserted: bool) -> Self {
        let at = Some(((seconds * NETWORK_FS) as u64, inserted));
        match dir {
            Direction::Down => self.slip_at = at,
            Direction::Up => self.up_slip_at = at,
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

    /// Downstream slips so far.
    pub fn slips(&self) -> u32 {
        self.slip_count
    }

    /// Downstream slips so far.
    pub fn slips_down(&self) -> u32 {
        self.slip_count
    }

    /// Upstream slips so far: only those the far end's buffer could make.
    pub fn slips_up(&self) -> u32 {
        self.up_slip_count
    }

    /// The last codeword the A/D made: its Ucode, and whether it was
    /// positive.
    ///
    /// A V.92 upstream receiver decides on codewords rather than on levels,
    /// and A-law has no exact zero, so a test that wants to know what arrived
    /// wants this rather than `up`.
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

    fn quantise(&self, level: f64) -> f64 {
        let (u, negative) = ucode::nearest(self.law, (level * 32768.0).round() as i32);
        ucode::level(self.law, u) * if negative { -1.0 } else { 1.0 }
    }

    /// What the network makes of one downstream level: the nearest codeword,
    /// through whatever the route does to it.
    fn carry(&mut self, level: f64) -> f64 {
        let (u, negative) = ucode::nearest(self.law, (level * 32768.0).round() as i32);
        let mut octet = ucode::octet(self.law, u, negative);
        if self.robbed == Some(self.octets % 6) {
            octet |= 1;
        }
        self.octets += 1;
        let (u, negative) = ucode::from_octet(self.law, octet);
        let level = ucode::level(self.law, u) * if negative { -1.0 } else { 1.0 };
        if self.pad == 1.0 { level } else { self.quantise(level * self.pad) }
    }

    /// What the network makes of one upstream sample the A/D has taken: the
    /// nearest codeword, through the upstream's own robbed bit and pad, and
    /// remembered as a codeword for `up_code`.
    fn carry_up(&mut self, level: f64) -> f64 {
        let (u, negative) = ucode::nearest(self.law, (level * 32768.0).round() as i32);
        let (u, negative) = match self.up_robbed {
            Some(phase) => {
                let mut octet = ucode::octet(self.law, u, negative);
                if phase == self.up_octets % 6 {
                    octet |= 1;
                }
                ucode::from_octet(self.law, octet)
            }
            None => (u, negative),
        };
        self.up_octets += 1;
        let level = ucode::level(self.law, u) * if negative { -1.0 } else { 1.0 };
        let (u, negative) = if self.up_pad == 1.0 {
            (u, negative)
        } else {
            ucode::nearest(self.law, (level * self.up_pad * 32768.0).round() as i32)
        };
        self.up_last = (u, !negative);
        ucode::level(self.law, u) * if negative { -1.0 } else { 1.0 }
    }

    /// One downstream level in, and whatever line samples the analogue modem
    /// hears by then out.
    pub fn down(&mut self, level: f64) -> Vec<f64> {
        let carried = self.carry(level);
        self.now += 1;
        // The jitter buffer, between the network and the sound card.
        let periodic = self.slips.filter(|(every, _)| self.now.is_multiple_of(*every));
        let once = self.slip_at.filter(|(at, _)| self.now == *at);
        if let Some((_, inserted)) = periodic.or(once) {
            self.slip_count += 1;
            if inserted {
                // Twenty milliseconds of the last twenty, fading: what packet
                // loss concealment makes up.
                let tail: Vec<f64> = self.recent.iter().copied().collect();
                for (k, v) in tail.iter().enumerate() {
                    self.down_levels.push_back(v * (1.0 - k as f64 / tail.len() as f64));
                }
            } else {
                self.dropping = self.slip_length;
            }
        }
        if self.dropping > 0 {
            self.dropping -= 1;
        } else {
            self.down_levels.push_back(carried);
            if self.recent.len() == self.slip_length {
                self.recent.pop_front();
            }
            self.recent.push_back(carried);
        }
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
            let mut heard = sum;
            if let Some((ceiling, release)) = self.gain_control {
                if (sum * self.gain).abs() > ceiling {
                    self.gain = ceiling / sum.abs();
                }
                heard = sum * self.gain;
                self.gain += (1.0 - self.gain) / (release * self.fs);
            }
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
            self.up_samples.extend(std::iter::repeat_n(0.0, (self.up_delay * self.fs).round() as usize));
            self.up_delay = 0.0;
        }
        let loop_noise = self.up_noise.unwrap_or(self.noise);
        for &x in samples {
            let noise = loop_noise * self.gaussian();
            self.up_samples.push_back(self.up_gain * x + noise);
        }
        let per = self.fs / ((1.0 + self.skew) * NETWORK_FS);
        let reach = UP_REACH * self.fs;
        // The far end's jitter buffer, which moves where in our waveform the
        // A/D is reading rather than what it reads.
        self.up_now += 1;
        let periodic = self.up_slips.filter(|(every, _)| self.up_now.is_multiple_of(*every));
        let once = self.up_slip_at.filter(|(at, _)| self.up_now == *at);
        if let Some((_, inserted)) = periodic.or(once) {
            self.slip_upstream(inserted, per, reach);
        }
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
        if self.quantised { self.carry_up(sum) } else { sum }
    }

    /// Codewords the far end's buffer is holding, over and above the filter's
    /// own reach: none unless it slips, and a slip's worth if it does.
    fn buffered(&self) -> usize {
        if self.up_slips.is_some() || self.up_slip_at.is_some() { self.slip_length } else { 0 }
    }

    /// The far end's jitter buffer slipping: made-up audio puts everything
    /// after it a slip's length later, and a lost stretch puts it that much
    /// earlier.
    ///
    /// Twenty milliseconds can only be thrown away by a buffer that is
    /// holding them, and here what it holds is the upstream leg's delay. A
    /// leg with nothing to give up cannot lose a stretch: the slip does not
    /// happen, and `slips_up` does not count it.
    fn slip_upstream(&mut self, inserted: bool, per: f64, reach: f64) {
        let length = self.slip_length as f64;
        if inserted {
            self.up_shift += length;
            self.up_slip_count += 1;
            return;
        }
        let ahead = (self.up_next - UP_LAG - (self.up_shift - length) + self.up_phase).max(0.0) * per;
        let newest = self.up_first + self.up_samples.len() as f64 - 1.0;
        if ahead + reach <= newest {
            self.up_shift -= length;
            self.up_slip_count += 1;
        }
    }

    /// The anti-alias filter's taps for a sampling instant `frac` of a line
    /// sample after a line sample, kept until that fraction moves.
    ///
    /// The taps are what the filter used to work out for every one of them on
    /// every network sample. With both clocks at the same rate the fraction
    /// never changes, so they are worked out once for a whole call; when the
    /// clocks differ the sampling instant drifts and they are rebuilt, which
    /// is what the filter did anyway.
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
    fn kernel_per_tap(history: &[f64], up_next: f64, fs: f64) -> f64 {
        let per = fs / NETWORK_FS;
        let reach = UP_REACH * fs;
        let lag = DOWN_REACH as f64 + 2.0 + UP_REACH * NETWORK_FS;
        let t = (up_next - lag).max(0.0) * per;
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
    #[test]
    fn the_tabulated_upstream_kernel_gives_the_same_levels_as_before() {
        let mut net = Network::new(Law::Mu, FS).with_upstream_gain(1.0).unquantised();
        let mut history: Vec<f64> = Vec::new();
        let mut seed = 0x1234_5678_9abc_def1u64;
        for n in 0..2000u64 {
            let pair = [random(&mut seed), random(&mut seed)];
            history.extend_from_slice(&pair);
            let got = net.up(&pair);
            let want = kernel_per_tap(&history, n as f64, FS);
            assert_eq!(got.to_bits(), want.to_bits(), "sample {n}: {got} against {want}");
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
    #[test]
    fn the_upstream_codeword_is_the_level_we_meant() {
        for law in [Law::Mu, Law::A] {
            let mut net = Network::new(law, FS).with_upstream_gain(1.0);
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
                    assert_eq!(net.up_code(), (u, positive), "{law:?} Ucode {u}, positive {positive}");
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
        // The anti-alias filter keeps about the band it passes, so a tenth of
        // the noise's power is still there.
        assert!((power / 2000.0).sqrt() > 5e-3, "{power}");
    }
}
