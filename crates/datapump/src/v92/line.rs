//! The V.92 analogue modem on a sound card: Phases 3 and 4 and data mode, at
//! the line's sample rate.
//!
//! [`super::analogue::Modem`] works a symbol at a time on both sides. Going
//! up that is what V.92 asks of it -- "The upstream symbol rate shall be 8000
//! symbol/s derived from the digital network" (6.2) -- and [`PcmTransmitter`]
//! turns those symbols into line samples. Coming down it reads one sample a
//! symbol, at the instant the far codec put the codeword on the line, and its
//! equaliser is T-spaced. So something has to say *which* line sample of
//! each symbol that is.
//!
//! On a sound card whose rate is a whole multiple of 8000 the answer is one of
//! a few whole sample phases, and Sd says which (8.4.4/V.90): it repeats
//! {+W, +0, +W, -W, -0, -W}, so on the right phase every third symbol is a
//! zero. The downstream is held back [`HOLD`] symbols while the live line is
//! watched for six-symbol repetition; when Sd is found, the phase whose
//! samples put the most of its energy away from one of the three positions a
//! zero can take is the one, and it is settled before the held-back stream
//! reaches Sd. The digital modem is silent from the end of Phase 2 until Sd
//! (9.5.1.1.2), so the step from the default phase lands in silence.
//!
//! What this does not do:
//!
//! * **Follow a clock.** The phase is chosen once. Between two of this
//!   project's own instances on virtual cables both ends run off one clock,
//!   and nothing drifts; a real line's 100 ppm walks a whole sample in a
//!   couple of seconds at 48 kHz, and wants a timing loop this does not have
//!   yet. [`SymbolClock`] stays nominal for the same reason.
//! * **Cancel echo.** A four-wire line has none; a two-wire one wants
//!   V92-43's canceller on this side too.
//! * **Any rate that is not a multiple of 8000.** There the downstream goes
//!   through a resampler at whatever phase it lands on, and the equaliser is
//!   left to make what it can of it.

use std::collections::VecDeque;

use dsp::Resampler;

use super::analogue::{Modem, Settings};
use super::epsilon;
use super::transmit::{Mode, PcmTransmitter};
use crate::v90::pcm::{BAUD, SymbolClock};

/// Symbols the downstream is held back while its phase is found: Sd has to
/// be seen [`PERIODS`] periods deep, [`SURE`] times over, before the held
/// stream reaches its first symbol.
const HOLD: usize = 48;

/// Sd's period in symbols (8.4.4/V.90).
const SD_PERIOD: usize = 6;

/// Periods of Sd each look compares, against the period before each.
const PERIODS: usize = 3;

/// The mean square below which a window is not looked at: the receiver's own
/// `AUDIBLE`, in the same units.
const AUDIBLE: f64 = 1e-4;

/// How alike each period must be to the one before it. Sd through a clean
/// line is alike to within its noise; the 1200 Hz of an INFO carrier or Tone
/// B comes to cos(2 pi 0.9) = 0.81, and 2400 Hz to 0.31.
const ALIKE: f64 = 0.95;

/// Looks in a row that must find Sd.
const SURE: u32 = 3;

/// Levels kept between the modem and the transmitter, so that a symbol the
/// transmitter asks for a sample early is never one the modem has not made.
const CUSHION: usize = 2;

/// The analogue modem at the line's rate.
#[derive(Debug, Clone)]
pub struct Line {
    modem: Modem,
    /// Line samples a symbol, when that is a whole number.
    per: Option<usize>,
    /// The way down when it is not.
    down: Option<Resampler>,
    arrived: Vec<f64>,
    /// The most recent line samples: what the modem has still to be given,
    /// and what Sd is looked for in.
    held: VecDeque<f64>,
    /// Line samples taken.
    taken: u64,
    /// The sample phase the downstream is read on, once Sd has said.
    phase: Option<usize>,
    /// Looks in a row that found Sd.
    alike: u32,
    tx: PcmTransmitter,
    /// Levels made and not yet asked for, each with the one-off step of the
    /// transmitter it is to go out after (zero for none).
    up: VecDeque<(f64, f64)>,
    /// Whether the modem has been given a symbol yet.
    started: bool,
    /// How many of [`Modem::delays`] the transmitter has taken.
    applied: usize,
    /// Steps the transmitter has refused, as last told.
    refused: u32,
    notes: Vec<String>,
}

impl Line {
    /// From the end of Phase 2, at `fs` samples a second.
    pub fn new(settings: Settings, fs: f64) -> Self {
        let ratio = fs / BAUD;
        let per = (ratio.fract().abs() < 1e-9 && ratio >= 1.0).then_some(ratio as usize);
        Self {
            modem: Modem::new(settings),
            per,
            down: per.is_none().then(|| Resampler::new(fs, BAUD)),
            arrived: Vec::with_capacity(4),
            held: VecDeque::new(),
            taken: 0,
            phase: None,
            alike: 0,
            tx: PcmTransmitter::new(fs, Mode::Interpolated),
            up: VecDeque::new(),
            started: false,
            applied: 0,
            refused: 0,
            notes: Vec::new(),
        }
    }

    pub fn modem(&self) -> &Modem {
        &self.modem
    }

    pub fn modem_mut(&mut self) -> &mut Modem {
        &mut self.modem
    }

    /// The downstream's sample phase, once Sd has given it.
    pub fn phase(&self) -> Option<usize> {
        self.phase
    }

    /// What the modem and this have done since this was last asked.
    pub fn take_notes(&mut self) -> Vec<String> {
        let mut notes = std::mem::take(&mut self.notes);
        notes.extend(self.modem.take_notes());
        notes
    }

    /// One line sample in, one out.
    pub fn step(&mut self, line: f64) -> f64 {
        match self.per {
            Some(per) => self.step_whole(line, per),
            None => {
                self.arrived.clear();
                if let Some(down) = self.down.as_mut() {
                    down.process(line, &mut self.arrived);
                }
                for i in 0..self.arrived.len() {
                    let level = self.arrived[i];
                    self.symbol(level);
                }
            }
        }
        let clock = SymbolClock {
            period: self.tx.period(),
            nominal: self.tx.period(),
            at: 0.0,
            index: 0,
            trained: false,
        };
        let up = &mut self.up;
        let out = self.tx.next_sample_stepped(clock, || up.pop_front().unwrap_or((0.0, 0.0)));
        if self.tx.refused() > self.refused {
            self.refused = self.tx.refused();
            self.notes.push("V.92 upstream: a third step of the transmitter was refused".into());
        }
        out
    }

    /// A line rate that is a whole number of samples a symbol.
    fn step_whole(&mut self, line: f64, per: usize) {
        self.held.push_back(line);
        let keep = (HOLD + (PERIODS + 1) * SD_PERIOD) * per + 1;
        while self.held.len() > keep {
            self.held.pop_front();
        }
        let n = self.taken;
        self.taken += 1;
        if self.phase.is_none() && n.is_multiple_of(per as u64) {
            self.look_for_sd(per);
        }
        // The sample HOLD symbols back, read on the chosen phase.
        let lag = HOLD * per;
        if let Some(m) = n.checked_sub(lag as u64)
            && m % per as u64 == self.phase.unwrap_or(0) as u64
            && self.held.len() > lag
        {
            let level = self.held[self.held.len() - 1 - lag];
            self.symbol(level);
        }
    }

    /// One symbol down, and the one it makes going up.
    fn symbol(&mut self, level: f64) {
        if !self.started {
            self.started = true;
            self.up.extend(std::iter::repeat_n((0.0, 0.0), CUSHION));
        }
        let out = self.modem.step(level);
        // 9.5.2.1.7-8: S-bar-u's half symbol, then epsilon, as lasting steps
        // of the transmitter, asked for with the S-bar-u's first symbol and
        // taken just before that symbol is placed -- not when the
        // transmitter next asks for a level, which is a few symbols of Su
        // earlier and is a transition the digital modem's refit is not
        // looking for.
        let delays = self.modem.delays();
        let step: f64 = delays[self.applied.min(delays.len())..].iter().map(|d| epsilon::step_direction() * d).sum();
        self.applied = delays.len();
        self.up.push_back((out, step));
    }

    /// Sd, in the live line: each of the last [`PERIODS`] six-symbol periods
    /// like the one before it, and loud enough to mean something.
    fn look_for_sd(&mut self, per: usize) {
        let span = SD_PERIOD * per;
        let window = PERIODS * span;
        if self.held.len() < window + span {
            return;
        }
        let end = self.held.len();
        let (mut now, mut before, mut both) = (0.0, 0.0, 0.0);
        for i in end - window..end {
            let (a, b) = (self.held[i], self.held[i - span]);
            now += a * a;
            before += b * b;
            both += a * b;
        }
        let found = now / window as f64 > AUDIBLE && both / (now * before).sqrt().max(1e-30) > ALIKE;
        self.alike = if found { self.alike + 1 } else { 0 };
        if self.alike >= SURE {
            let phase = self.sd_phase(per);
            self.phase = Some(phase);
            self.notes.push(format!("V.92: Sd found; the downstream is read on sample {phase} of every {per}"));
        }
    }

    /// The sample phase on which the Sd in hand is Sd: for each, the six
    /// symbols it reads averaged over the periods held, and how nearly they
    /// are {+W, 0, +W, -W, 0, -W} turned round to whichever symbol is first.
    ///
    /// The shape and not merely its zeros. Two thirds of Sd's power is at
    /// 4000 Hz, and read half a symbol out that part is read at its own zero
    /// crossings and vanishes; what is left is the 1333 Hz fundamental, which
    /// read there is {+, 0, -, -, 0, +} -- a zero every third symbol, exactly
    /// as on the right phase. The two differ in the pair between the zeros,
    /// and a correlation with the whole shape sees it: one on the right
    /// phase, at most the fundamental's share, 0.58, half a symbol out.
    fn sd_phase(&self, per: usize) -> usize {
        const SD: [f64; SD_PERIOD] = [1.0, 0.0, 1.0, -1.0, 0.0, -1.0];
        let span = SD_PERIOD * per;
        let length = (PERIODS + 1) * span;
        let end = self.held.len();
        // The absolute index of held[0], so that every phase is counted from
        // the same sample whatever has been dropped.
        let first = self.taken - end as u64;
        let mut best = (f64::NEG_INFINITY, 0);
        for phase in 0..per {
            let mut sums = [0.0f64; SD_PERIOD];
            for i in end - length..end {
                let at = first + i as u64;
                if at % per as u64 != phase as u64 {
                    continue;
                }
                let symbol = (at / per as u64) as usize % SD_PERIOD;
                sums[symbol] += self.held[i];
            }
            let total: f64 = sums.iter().map(|s| s * s).sum();
            if total <= 0.0 {
                continue;
            }
            let norm = (total * SD.iter().map(|x| x * x).sum::<f64>()).sqrt();
            let score = (0..SD_PERIOD)
                .map(|turn| (0..SD_PERIOD).map(|k| sums[k] * SD[(k + turn) % SD_PERIOD]).sum::<f64>().abs() / norm)
                .fold(0.0, f64::max);
            if score > best.0 {
                best = (score, phase);
            }
        }
        best.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::sequences::Descriptor;
    use crate::v90::ucode::Law;
    use crate::v92::FilterLimits;
    use crate::v92::analogue::Status as AStatus;
    use crate::v92::digital;

    /// The digital modem of `v90::server::Line`: at the network's rate, with
    /// the line's rate each side of it through the same resamplers.
    struct Server {
        modem: digital::Modem,
        down: Resampler,
        up: Resampler,
        arrived: Vec<f64>,
        made: Vec<f64>,
        out: VecDeque<f64>,
    }

    impl Server {
        fn new(fs: f64) -> Self {
            let settings = digital::Settings {
                law: Law::Mu,
                uinfo: 79,
                far_md: 0,
                round_trip: 0.0,
                wide: true,
                limits: FilterLimits::from_info1a(0, 0, 0),
                habits: digital::Habits::PROMPT,
            };
            Self {
                modem: digital::Modem::new(settings),
                down: Resampler::new(fs, digital::FS),
                up: Resampler::new(digital::FS, fs),
                arrived: Vec::new(),
                made: Vec::new(),
                out: VecDeque::from(vec![0.0; 8]),
            }
        }

        fn step(&mut self, line: f64) -> f64 {
            self.arrived.clear();
            self.down.process(line, &mut self.arrived);
            for i in 0..self.arrived.len() {
                let level = self.modem.step(self.arrived[i]);
                self.made.clear();
                self.up.process(level, &mut self.made);
                self.out.extend(self.made.iter().copied());
            }
            while self.out.len() > 16 {
                self.out.pop_front();
            }
            self.out.pop_front().unwrap_or(0.0)
        }
    }

    fn settings() -> Settings {
        Settings::new(Descriptor::none(), 0x7ffff, super::super::analogue::readable_cpt())
    }

    /// The two ends at `fs`, `down` and `up` line samples apart each way, run
    /// until both are in data mode. What the analogue end said, and whether
    /// they got there.
    fn call(fs: f64, down: usize, up: usize) -> (bool, Line, String) {
        let mut server = Server::new(fs);
        let mut line = Line::new(settings(), fs);
        let mut to_analogue = VecDeque::from(vec![0.0; down.max(1)]);
        let mut to_server = VecDeque::from(vec![0.0; up.max(1)]);
        let mut said = String::new();
        let mut done = false;
        for _ in 0..(20.0 * fs) as usize {
            let heard = to_analogue.pop_front().unwrap();
            to_server.push_back(line.step(heard));
            let heard = to_server.pop_front().unwrap();
            to_analogue.push_back(server.step(heard));
            for note in line.take_notes() {
                said.push_str(&note);
                said.push('\n');
            }
            for note in server.modem.take_notes() {
                said.push_str("    digital: ");
                said.push_str(&note);
                said.push('\n');
            }
            let both = matches!(line.modem().status(), AStatus::Connected { .. })
                && matches!(server.modem.status(), digital::Status::Connected { .. });
            if both {
                done = true;
                break;
            }
            if matches!(line.modem().status(), AStatus::Failed(_) | AStatus::Retrain(_)) {
                break;
            }
        }
        said.push_str(&format!("analogue {:?}, digital {:?}, shifts {} refused {} delays {:?}", line.modem().status(), server.modem.status(), line.tx.shifts(), line.tx.refused(), line.modem().delays()));
        (done, line, said)
    }

    /// Two of this project's own ends on a pair of virtual cables at 48 kHz:
    /// both in data mode, whatever whole number of samples each way puts the
    /// downstream's codewords on.
    #[test]
    fn a_whole_v92_call_at_48_khz_comes_up_at_every_sample_phase() {
        let mut failed = Vec::new();
        for down in 240..246 {
            for up in 240..246 {
                let (done, line, said) = call(48_000.0, down, up);
                if !done || line.phase().is_none() {
                    eprintln!("{down}/{up} failed: {}", said.lines().last().unwrap_or(""));
                    failed.push((down, up, said));
                }
            }
        }
        if let Some((down, up, said)) = failed.first() {
            panic!("{} of 36 failed; the first, {down}/{up}:\n{said}", failed.len());
        }
    }
}
