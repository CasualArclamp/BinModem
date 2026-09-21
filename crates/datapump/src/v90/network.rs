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
//! drops twenty, which shifts everything after it by 160 codewords -- or by
//! however many a packet holds ([`Network::with_slips_of`]).
//!
//! A packet can also be lost and concealed where it was, with nothing moved
//! at all ([`Network::with_dropout`]): the buffer plays what it made up in
//! the place the lost packet would have filled, and everything after it stays
//! exactly where it was.
//!
//! And a path can take the top of the band away: something between the
//! network and the sound card -- a transcoder's filter, a resampler -- that
//! passes everything to 3.6 kHz and next to nothing at 4. See
//! [`Network::with_band_edge_cut`].
//!
//! And a line can be disturbed in the middle of a call: noise that comes and
//! goes ([`Network::with_bursts`]), and a floor that steps up or creeps up
//! ([`Network::with_rising_noise`]).
//!
//! Nothing here is a claim about any real network, only about what V.90 has
//! to get through.

use std::collections::VecDeque;

use super::ucode::{self, Law};

/// The network's rate.
const NETWORK_FS: f64 = 8000.0;

/// The band-edge cut: a windowed sinc at this frequency, reaching this far
/// either side, in seconds of line. With the codec's own reconstruction in
/// front of it the whole path is flat to 3.5 kHz, 1 dB down at 3.6 kHz, 10 dB
/// at 3.8, 26 at 3.9 and 48 at 3.975.
const CUT_HZ: f64 = 3830.0;
const CUT_REACH: f64 = 0.006;

/// Codewords either side of an instant the codec's reconstruction reaches:
/// short, since a reconstruction filter that rings on for longer than an
/// equaliser reaches is not one any codec has.
const DOWN_REACH: i64 = 20;

/// Line samples either side the codec's anti-alias filter reaches, in
/// seconds of line: long, since the upstream's band runs close to 4 kHz.
const UP_REACH: f64 = 0.008;

/// A slip's length, in codewords: one twenty-millisecond packet.
pub const SLIP: usize = 160;

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
    /// Network time: codewords sent downstream so far.
    now: u64,
    noise: f64,
    seed: u64,
    /// Which of the six octets a robbed bit lands on, if one does.
    robbed: Option<usize>,
    octets: usize,
    /// A digital pad, as a gain on every downstream level.
    pad: f64,
    /// Whether the upstream is quantised to G.711.
    quantised: bool,
    /// What the analogue modem's line level is to the codec's full scale.
    ///
    /// Every modem here leaves at a root-mean-square of 0.707, a full-scale
    /// sine, and a codec quantising that clips every peak. A telephone line
    /// delivers a modem's -9 to -12 dBm to the codec well inside its range;
    /// this is where that happens.
    up_gain: f64,
    /// Downstream slips: how often, and whether audio is made up or lost;
    /// or one, at a given codeword. `slip_length` codewords go or come.
    slips: Option<(u64, bool)>,
    slip_at: Option<(u64, bool)>,
    slip_length: usize,
    /// Codewords of a lost stretch still to drop.
    dropping: usize,
    /// Codewords kept for concealment to repeat, and how many of them.
    recent: VecDeque<f64>,
    kept: usize,
    /// The level the comfort noise of the dropout under way is made at.
    comfort: f64,
    slip_count: u32,
    /// A softphone's gain control on what it plays: the loudest it lets
    /// through, how fast it recovers, in seconds, and where it has got to.
    gain_control: Option<(f64, f64)>,
    gain: f64,
    /// The band-edge cut's taps, and the line samples they reach over,
    /// newest first.
    cut: Option<(Vec<f64>, VecDeque<f64>)>,
    /// Bursts of noise on the downstream, and a floor that rises.
    bursts: Option<Bursts>,
    rising: Option<Rising>,
    /// Packets lost and concealed where they were.
    dropout: Option<Dropout>,
}

/// A packet lost and concealed in place: from codeword `from` on, `length`
/// codewords of what the buffer makes up every `every` codewords, either a
/// fading repeat of the last packet or nothing at all.
#[derive(Debug, Clone, Copy)]
struct Dropout {
    from: u64,
    every: u64,
    length: u64,
    repeat: bool,
}

impl Dropout {
    /// How far into a dropout network time `now` is, if it is in one.
    fn within(&self, now: u64) -> Option<u64> {
        let since = now.checked_sub(self.from)?;
        let k = since % self.every;
        (k < self.length).then_some(k)
    }
}

/// Noise that comes and goes: from codeword `from` on, `length` codewords of
/// it at `level` every `every` codewords.
#[derive(Debug, Clone, Copy)]
struct Bursts {
    from: u64,
    every: u64,
    length: u64,
    level: f64,
}

impl Bursts {
    /// The burst's level at network time `now`: nothing between bursts.
    fn level(&self, now: u64) -> f64 {
        match now.checked_sub(self.from) {
            Some(since) if since % self.every < self.length => self.level,
            _ => 0.0,
        }
    }
}

/// A floor that rises: from codeword `from` on, from wherever it was then to
/// `end` over `over` codewords, by as many decibels each codeword.
#[derive(Debug, Clone, Copy)]
struct Rising {
    from: u64,
    over: u64,
    end: f64,
    /// The floor when the rise began.
    start: Option<f64>,
}

impl Rising {
    /// The floor at network time `now`, if the rise has begun, from a floor
    /// that was `floor` before it.
    fn level(&mut self, now: u64, floor: f64) -> Option<f64> {
        let since = now.checked_sub(self.from)?;
        let start = *self.start.get_or_insert(floor);
        if since >= self.over || start <= 0.0 {
            return Some(self.end);
        }
        Some(start * (self.end / start).powf(since as f64 / self.over as f64))
    }
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
            noise: 0.0,
            seed: 0x2545_f491_4f6c_dd1d,
            robbed: None,
            octets: 0,
            pad: 1.0,
            quantised: true,
            up_gain: 0.25,
            slips: None,
            slip_at: None,
            slip_length: SLIP,
            dropping: 0,
            recent: VecDeque::with_capacity(SLIP),
            kept: SLIP,
            comfort: 0.0,
            slip_count: 0,
            gain_control: None,
            gain: 1.0,
            cut: None,
            bursts: None,
            rising: None,
            dropout: None,
        }
    }

    /// Each way's delay, in seconds of line.
    pub fn with_delay(mut self, seconds: f64, _fs: f64) -> Self {
        self.down_delay = seconds;
        self.up_delay = seconds;
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

    /// The loop's noise from now on: a line that goes bad in the middle of a
    /// call.
    pub fn set_noise(&mut self, level: f64) {
        self.noise = level;
    }

    /// The loop's noise rising to `level` from `from` seconds into the call,
    /// over `over` seconds -- at once, if that is nothing -- by as many
    /// decibels each second: a line that goes bad in the middle of a call,
    /// all at once or a little at a time.
    pub fn with_rising_noise(mut self, from: f64, over: f64, level: f64) -> Self {
        let codewords = |seconds: f64| (seconds * NETWORK_FS) as u64;
        self.rising = Some(Rising { from: codewords(from), over: codewords(over), end: level, start: None });
        self
    }

    /// Bursts of noise on the downstream as the analogue modem hears it, from
    /// `from` seconds into the call: `length` seconds at `level` on top of the
    /// floor, every `every` seconds.
    ///
    /// A disturbance that comes and goes -- a crackle on the loop, a noisy
    /// neighbour in the cable -- in the downstream only, as the rest of what
    /// this route does to what the analogue modem hears is: the upstream is
    /// V.34's, with margins and a receiver of its own, and what is asked of
    /// these is what the downstream's receiver makes of them.
    pub fn with_bursts(mut self, from: f64, every: f64, length: f64, level: f64) -> Self {
        let codewords = |seconds: f64| (seconds * NETWORK_FS) as u64;
        self.bursts = Some(Bursts { from: codewords(from), every: codewords(every).max(1), length: codewords(length), level });
        self
    }

    /// A robbed bit on every sixth downstream octet, starting at `phase`.
    pub fn with_robbed_bit(mut self, phase: usize) -> Self {
        self.robbed = Some(phase % 6);
        self
    }

    /// A digital pad of `db` on the downstream.
    pub fn with_pad(mut self, db: f64) -> Self {
        self.pad = 10f64.powf(-db / 20.0);
        self
    }

    /// The analogue modem's level at the codec, against its own.
    pub fn with_upstream_gain(mut self, gain: f64) -> Self {
        self.up_gain = gain;
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
    pub fn with_slips(mut self, seconds: f64, inserted: bool) -> Self {
        self.slips = Some(((seconds * NETWORK_FS) as u64, inserted));
        self
    }


    /// The same, of `codewords` in place of a twenty-millisecond packet's
    /// 160: a buffer moves whole packets, and a packet holds ten, twenty or
    /// thirty milliseconds of G.711 -- 80, 160 or 240 codewords.
    ///
    /// The length matters to more than the length. V.90's frames are six
    /// codewords (7.1), so a slip of a multiple of six leaves every frame
    /// where it was in the frame grid and the receiver never has to find its
    /// place again: 240 codewords move everything on by thirty milliseconds
    /// and say nothing else about it at all.
    pub fn with_slips_of(mut self, seconds: f64, codewords: usize, inserted: bool) -> Self {
        self.slip_length = codewords;
        self.kept = self.kept.max(codewords);
        self.with_slips(seconds, inserted)
    }

    /// A packet of the downstream lost and concealed where it was: from
    /// `from` seconds on, `length` seconds of it every `every` seconds,
    /// replaced by what the buffer makes up -- the last packet over again,
    /// fading, if `repeat`, as a concealer that repeats a pitch period does,
    /// and noise at the level of the packet that was lost if not, as a buffer
    /// with comfort noise does. Garbage to a modem either way.
    ///
    /// Nothing is inserted and nothing is dropped, so everything after it is
    /// exactly where it always was: the clock does not shift, the frames do
    /// not move, and only the made-up audio itself says anything happened.
    /// This is what a packet lost on a VoIP leg looks like whenever the
    /// buffer has time to conceal it rather than resynchronise -- the usual
    /// case for a single loss, and the one Rory's line gives.
    pub fn with_dropout(mut self, from: f64, every: f64, length: f64, repeat: bool) -> Self {
        let codewords = |seconds: f64| (seconds * NETWORK_FS) as u64;
        let length = codewords(length);
        self.kept = self.kept.max(length as usize);
        self.dropout = Some(Dropout { from: codewords(from), every: codewords(every).max(1), length, repeat });
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

    /// The top of the downstream's band taken away, as the analogue modem
    /// hears it: flat to 3.6 kHz, about 10 dB down at 3.8 and 48 dB down just
    /// short of 4.
    ///
    /// What a live call over a VoIP provider did to our own digital modem's
    /// TRN1d and Jd (live-1789732858, 16.3 to 17.5 s): flat to 3.5 kHz, 2 dB
    /// down at 3.6 to 3.7, 6 at 3.75, 11 at 3.8, 22 at 3.9 and 36 at 4.0.
    /// TRN1d is as good as white, so that is the path's own shape. The cut
    /// here, with the codec's reconstruction in front of it, is that path and
    /// a little deeper at the very top. It is linear in phase, as a
    /// resampler's filter is, and delays everything by [`CUT_REACH`].
    pub fn with_band_edge_cut(mut self) -> Self {
        let reach = (CUT_REACH * self.fs).round() as i64;
        let mut taps: Vec<f64> = (-reach + 1..reach).map(|n| kernel(n as f64, CUT_HZ / self.fs, reach as f64)).collect();
        // Unit gain at DC, which the taps come to only approximately.
        let sum: f64 = taps.iter().sum();
        for tap in &mut taps {
            *tap /= sum;
        }
        let kept = VecDeque::from(vec![0.0; taps.len()]);
        self.cut = Some((taps, kept));
        self
    }

    /// One downstream slip, `seconds` into the call.
    pub fn with_slip_at(mut self, seconds: f64, inserted: bool) -> Self {
        self.slip_at = Some(((seconds * NETWORK_FS) as u64, inserted));
        self
    }

    /// Slips so far.
    pub fn slips(&self) -> u32 {
        self.slip_count
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

    /// The `k`th codeword of what a buffer makes up in place of a packet
    /// `length` codewords long: the last packet that arrived, over again,
    /// and silence if none has.
    fn made_up(&self, length: usize, k: usize) -> f64 {
        let from = self.recent.len().saturating_sub(length);
        self.recent.get(from + k).copied().unwrap_or(0.0)
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

    /// One downstream level in, and whatever line samples the analogue modem
    /// hears by then out.
    pub fn down(&mut self, level: f64) -> Vec<f64> {
        let carried = self.carry(level);
        // The disturbances as they stand for this codeword, the first being
        // codeword 0.
        let floor = self.noise;
        if let Some(level) = self.rising.as_mut().and_then(|r| r.level(self.now, floor)) {
            self.noise = level;
        }
        let burst = self.bursts.map_or(0.0, |b| b.level(self.now));
        let concealed = self.dropout.and_then(|d| d.within(self.now).map(|k| (d, k)));
        self.now += 1;
        // The jitter buffer, between the network and the sound card.
        let periodic = self.slips.filter(|(every, _)| self.now.is_multiple_of(*every));
        let once = self.slip_at.filter(|(at, _)| self.now == *at);
        if let Some((_, inserted)) = periodic.or(once) {
            self.slip_count += 1;
            if inserted {
                // A packet's worth of the last packet, fading: what packet
                // loss concealment makes up.
                for k in 0..self.slip_length {
                    let made_up = self.made_up(self.slip_length, k);
                    self.down_levels.push_back(made_up * (1.0 - k as f64 / self.slip_length as f64));
                }
            } else {
                self.dropping = self.slip_length;
            }
        }
        if self.dropping > 0 {
            self.dropping -= 1;
        } else if let Some((dropout, k)) = concealed {
            // A packet that never came, concealed in the place it would have
            // filled: the same made-up audio as a slip's, but instead of the
            // codewords rather than as well as them. Nothing goes into
            // `recent`, so what is repeated is the last packet that arrived.
            let length = dropout.length as usize;
            if k == 0 && !dropout.repeat {
                let power: f64 = (0..length).map(|j| self.made_up(length, j)).map(|v| v * v).sum();
                self.comfort = (power / length as f64).sqrt();
            }
            let made_up = if dropout.repeat {
                self.made_up(length, k as usize) * (1.0 - k as f64 / dropout.length as f64)
            } else {
                self.comfort * self.gaussian()
            };
            self.down_levels.push_back(made_up);
        } else {
            self.down_levels.push_back(carried);
            if self.recent.len() == self.kept {
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
            if let Some((taps, kept)) = self.cut.as_mut() {
                kept.pop_back();
                kept.push_front(sum);
                sum = taps.iter().zip(kept.iter()).map(|(h, x)| h * x).sum();
            }
            let mut heard = sum;
            if let Some((ceiling, release)) = self.gain_control {
                if (sum * self.gain).abs() > ceiling {
                    self.gain = ceiling / sum.abs();
                }
                heard = sum * self.gain;
                self.gain += (1.0 - self.gain) / (release * self.fs);
            }
            // A burst is noise of its own on top of the floor's.
            let level = if burst > 0.0 { self.noise.hypot(burst) } else { self.noise };
            let noise = level * self.gaussian();
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
        for &x in samples {
            let noise = self.noise * self.gaussian();
            self.up_samples.push_back(self.up_gain * x + noise);
        }
        // This network sample's instant, in the analogue modem's samples:
        // late enough that the modem has said everything the filter reaches,
        // since what it says answers a downstream that is itself late by the
        // reconstruction's reach.
        let per = self.fs / ((1.0 + self.skew) * NETWORK_FS);
        let reach = UP_REACH * self.fs;
        let lag = DOWN_REACH as f64 + 2.0 + UP_REACH * NETWORK_FS;
        let t = (self.up_next - lag).max(0.0) * per;
        self.up_next += 1.0;
        let centre = t.floor() as i64;
        let cutoff = 3700.0 / self.fs;
        let mut sum = 0.0;
        for j in centre - reach as i64..=centre + reach as i64 {
            let index = j as f64 - self.up_first;
            if index < 0.0 {
                continue;
            }
            let Some(&v) = self.up_samples.get(index as usize) else { continue };
            sum += v * kernel(t - j as f64, cutoff, reach + 1.0);
        }
        while self.up_first + reach + 2.0 < t && self.up_samples.len() > 1 {
            self.up_samples.pop_front();
            self.up_first += 1.0;
        }
        if self.quantised { self.quantise(sum) } else { sum }
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


    /// A slip of a chosen length moves everything after it by that length and
    /// by nothing else: 240 codewords, a thirty-millisecond packet, by thirty
    /// milliseconds.
    #[test]
    fn a_slip_of_a_chosen_length_moves_everything_after_it_by_that_much() {
        for (codewords, inserted) in [(240, true), (240, false), (80, true), (80, false)] {
            let mut net = Network::new(Law::Mu, 16_000.0).with_slips_of(1.0, codewords, inserted);
            let mut heard = 0usize;
            for _ in 0..12_000 {
                heard += net.down(0.1).len();
            }
            assert_eq!(net.slips(), 1);
            // Two line samples a codeword at 16 kHz.
            let moved = 2 * codewords as i64 * if inserted { 1 } else { -1 };
            assert!((heard as i64 - (24_000 + moved)).abs() < 60, "{codewords}, inserted {inserted}: {heard}");
        }
    }

    /// What the analogue modem hears of a downstream whose codewords change
    /// every time, over `seconds`.
    fn heard_codewords(mut net: Network, seconds: f64) -> Vec<f64> {
        let mut out = Vec::new();
        let mut u = 0u8;
        for _ in 0..(seconds * NETWORK_FS) as usize {
            u = u.wrapping_add(37);
            out.extend(net.down(ucode::level(Law::Mu, u % 128) * if u.is_multiple_of(2) { 1.0 } else { -1.0 }));
        }
        out
    }

    /// A packet lost and concealed in place changes the audio where it was
    /// lost and nowhere else: the same number of samples come out, at the
    /// same instants, and everything outside the lost packet is sample for
    /// sample what it would have been.
    #[test]
    fn a_concealed_dropout_changes_the_audio_where_it_was_and_moves_nothing() {
        let plain = heard_codewords(Network::new(Law::Mu, 16_000.0), 1.0);
        for repeat in [true, false] {
            let net = Network::new(Law::Mu, 16_000.0).with_dropout(0.5, 1.0, 0.02, repeat);
            let dropped = heard_codewords(net, 1.0);
            assert_eq!(dropped.len(), plain.len(), "repeat {repeat}: the dropout moved what came after it");
            let differs: Vec<usize> =
                plain.iter().zip(&dropped).enumerate().filter(|(_, (a, b))| (*a - *b).abs() > 1e-9).map(|(k, _)| k).collect();
            // Codewords 4000 to 4160 are line samples 8000 to 8320 at 16 kHz,
            // and the codec's reconstruction reaches DOWN_REACH either side.
            let reach = 2 * DOWN_REACH as usize;
            let (first, last) = (differs[0], differs[differs.len() - 1]);
            println!("repeat {repeat}: {} samples differ, {first} to {last}", differs.len());
            assert!(differs.len() > 300, "repeat {repeat}: only {} samples differ", differs.len());
            assert!(first > 8000 - reach && last < 8320 + reach, "repeat {repeat}: {first} to {last}");
        }
    }

    /// A tone's level as the analogue modem hears it, against the level it
    /// was sent at, in decibels.
    fn heard_db(mut net: Network, hz: f64) -> f64 {
        let amplitude = 0.3;
        let mut heard = Vec::new();
        for n in 0..24_000 {
            heard.extend(net.down(amplitude * (2.0 * std::f64::consts::PI * hz * n as f64 / NETWORK_FS).sin()));
        }
        // A second's worth from the middle, correlated against the tone:
        // quantising's noise is spread across the band, and falls away.
        let (mut i, mut q) = (0.0, 0.0);
        let from = heard.len() / 3;
        for (k, x) in heard[from..from + 16_000].iter().enumerate() {
            let phase = 2.0 * std::f64::consts::PI * hz * k as f64 / 16_000.0;
            i += x * phase.cos();
            q += x * phase.sin();
        }
        let level = 2.0 * (i * i + q * q).sqrt() / 16_000.0;
        20.0 * (level / amplitude).log10()
    }

    /// The live path's shape (live-1789732858): next to nothing lost to 3.6
    /// kHz, about ten decibels at 3.8, and next to everything just short of 4.
    #[test]
    fn a_band_edge_cut_keeps_the_band_and_takes_its_top() {
        let cut = || Network::new(Law::Mu, 16_000.0).with_band_edge_cut();
        let at = |hz: f64| heard_db(cut(), hz);
        let (low, mid, edge, top) = (at(1000.0), at(3600.0), at(3800.0), at(3975.0));
        println!("1000 Hz {low:.1} dB, 3600 Hz {mid:.1}, 3800 Hz {edge:.1}, 3975 Hz {top:.1}");
        assert!(low.abs() < 0.2, "1000 Hz {low:.1} dB");
        assert!(mid > -2.0, "3600 Hz {mid:.1} dB");
        assert!((-12.0..-8.0).contains(&edge), "3800 Hz {edge:.1} dB");
        assert!(top < -45.0, "3975 Hz {top:.1} dB");
        // And without it, the reconstruction's own edge is gentler.
        assert!(heard_db(Network::new(Law::Mu, 16_000.0), 3975.0) > -20.0);
    }

    /// What the analogue modem hears of a silent downstream, as the RMS over
    /// each tenth of a second, for `seconds`.
    fn noise_heard(mut net: Network, seconds: f64) -> Vec<f64> {
        let tenth = (NETWORK_FS / 10.0) as usize;
        (0..(seconds * 10.0) as usize)
            .map(|_| {
                let heard: Vec<f64> = (0..tenth).flat_map(|_| net.down(0.0)).collect();
                (heard.iter().map(|x| x * x).sum::<f64>() / heard.len() as f64).sqrt()
            })
            .collect()
    }

    /// Bursts are there for as long as they were asked to be and at the level
    /// asked for, on top of the floor, and the floor alone between them.
    #[test]
    fn noise_bursts_come_and_go_on_top_of_the_floor() {
        let net = Network::new(Law::Mu, 16_000.0).with_noise(1e-4).with_bursts(0.5, 1.0, 0.2, 1e-3);
        let heard = noise_heard(net, 3.0);
        let with_burst = 1e-4f64.hypot(1e-3);
        for (tenth, rms) in heard.iter().enumerate() {
            // The noise goes on after the codec's reconstruction, so a burst
            // begins and ends where it was asked to, on tenths here.
            let bursting = tenth >= 5 && (tenth - 5) % 10 < 2;
            let expected = if bursting { with_burst } else { 1e-4 };
            assert!((rms / expected - 1.0).abs() < 0.1, "tenth {tenth}: {rms:.2e} against {expected:.2e}");
        }
    }

    /// A floor that rises goes from where it was to where it was asked to,
    /// by as many decibels each second, and stays there.
    #[test]
    fn a_rising_floor_rises_by_the_same_decibels_each_second() {
        let net = Network::new(Law::Mu, 16_000.0).with_noise(1e-5).with_rising_noise(1.0, 2.0, 1e-3);
        let heard = noise_heard(net, 4.0);
        // Tenths 0 to 9 before the rise, 10 to 29 during it, 30 on after.
        let db = |rms: f64| 20.0 * rms.log10();
        assert!((db(heard[5]) - db(1e-5)).abs() < 1.0, "before: {:.2e}", heard[5]);
        // Half way through, half way in decibels: the middle of the tenth
        // that starts it is 2.05 s, a fortieth of the way on from 2.0.
        let middle = db(1e-5) + (db(1e-3) - db(1e-5)) * 0.525;
        assert!((db(heard[20]) - middle).abs() < 1.0, "half way: {:.1} dB against {middle:.1}", db(heard[20]));
        assert!((db(heard[35]) - db(1e-3)).abs() < 1.0, "after: {:.2e}", heard[35]);
        // It is the loop's floor, as `set_noise` sets it: the upstream hears
        // it too.
        let mut net = Network::new(Law::Mu, 16_000.0).with_noise(1e-5).with_rising_noise(0.0, 0.0, 1e-3);
        let _ = net.down(0.0);
        assert_eq!(net.noise, 1e-3);
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
}
