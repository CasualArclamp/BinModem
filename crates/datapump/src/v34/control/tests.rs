//! Both ends of a control channel on one pair of wires.
//!
//! The line is two-wire: each end hears the far end and its own signal
//! coming back, the answer modem's guard tone with it. Delay each way, a
//! different loss each way, noise, the answer end's clock off the call
//! end's by resampling both ways (as `training.rs`'s `Link` has it), and a
//! VoIP jitter buffer's 20 ms slip.
//!
//! Each end is driven by a stand-in for 12.4's procedure, just enough to
//! carry it from silence to data: the source sends PPh first; the recipient
//! answers PPh with its own; each sends its MPh repeated once it has PPh, and
//! E once it has read the far end's MPh; then data. The real procedures are
//! package G's.

use std::collections::VecDeque;

use super::*;
use crate::v34::mp::Mp;

const FS: f64 = 16_000.0;

fn contains(haystack: &[bool], needle: &[bool]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// A pattern of `length` bits no idle line of ones could be mistaken for.
fn pattern(length: usize, seed: u32) -> Vec<bool> {
    let mut state = seed.max(1);
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state & 1 == 1
        })
        .collect()
}

/// What an end is doing, in the stand-in procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The source waits for the recipient's PPh; the recipient for the
    /// source's.
    Waiting,
    /// Sending MPh repeatedly, until the far end's has been read.
    Mph,
    /// E queued, and data after it.
    Data,
    /// Sent Sh and S-bar-h, waiting for the far end's (12.6.1.2); or, as the
    /// recipient, waiting for them to answer.
    Resync,
    /// A control-channel retrain (12.8): the initiator sends AC and waits
    /// for PPh; the responder answers AC heard for 100 ms with PPh and ALT,
    /// then waits for the initiator's.
    Retrain { initiator: bool, answered: bool },
}

/// One end: the modem and the stand-in procedure driving it.
#[derive(Debug)]
struct End {
    modem: Modem,
    source: bool,
    reading: Reading,
    mph: Vec<bool>,
    far_mph: Vec<bool>,
    stage: Stage,
    sync: Vec<bool>,
    data: Vec<bool>,
    /// Everything heard, with the sample it was reported at.
    heard: Vec<(u64, Heard)>,
}

impl End {
    fn new(side: Side, source: bool, rate: Rate, reading: Reading) -> Self {
        let mut modem = Modem::new(side, FS);
        modem.receiver.set_rate(rate);
        modem.transmitter.set_rate(rate);
        let mph_of = |side: Side| {
            let mp = Mp {
                call_to_answer: if side == Side::Call { 14 } else { 11 },
                answer_to_call: 9,
                rates: Mp::rates_up_to(12),
                ..Mp::default()
            };
            mp.to_bits()
        };
        let mut end = Self {
            modem,
            source,
            reading,
            mph: mph_of(side),
            far_mph: mph_of(far(side)),
            stage: Stage::Waiting,
            sync: Vec::new(),
            data: Vec::new(),
            heard: Vec::new(),
        };
        if source {
            // 12.4.1.1: 70 ms of silence, PPh, and ALT for at least 16T.
            let tx = &mut end.modem.transmitter;
            tx.queue(Segment::Silence(SILENCE_SYMBOLS));
            tx.queue(Segment::Pph(reading));
            tx.queue(Segment::Alt { at_least: ALT_LEAST });
        }
        end
    }

    fn step(&mut self, input: f64) -> f64 {
        let out = self.modem.step(input);
        while let Some(heard) = self.modem.receiver.heard() {
            self.heard.push((self.modem.receiver.now(), heard));
            match heard {
                Heard::Pph { .. } if matches!(self.stage, Stage::Retrain { .. }) => {
                    let tx = &mut self.modem.transmitter;
                    if let Stage::Retrain { initiator: true, .. } = self.stage {
                        // 12.8.1: PPh, ALT for 16T to 120T, MPh.
                        tx.queue(Segment::Pph(self.reading));
                        tx.queue(Segment::Alt { at_least: ALT_LEAST });
                    }
                    tx.queue(Segment::Repeat(self.mph.clone()));
                    self.stage = Stage::Mph;
                }
                Heard::Pph { .. } if self.stage == Stage::Waiting => {
                    let tx = &mut self.modem.transmitter;
                    if !self.source {
                        // 12.4.2.1 and 12.4.2.2: PPh, then ALT.
                        tx.queue(Segment::Pph(self.reading));
                        tx.queue(Segment::Alt { at_least: ALT_LEAST });
                    }
                    // MPh as soon as ALT allows (12.4.1.2, 12.4.2.3).
                    tx.queue(Segment::Repeat(self.mph.clone()));
                    self.stage = Stage::Mph;
                }
                Heard::Reversal { .. } if self.stage == Stage::Resync => {
                    let tx = &mut self.modem.transmitter;
                    if !self.source {
                        // 12.6.2.2: Sh and S-bar-h back, ALT, E.
                        tx.queue(Segment::Sh(SH_SYMBOLS));
                        tx.queue(Segment::ShBar(SH_BAR_SYMBOLS));
                        tx.queue(Segment::Alt { at_least: ALT_LEAST });
                    }
                    // 12.6.1.4: ALT for 16T at least, then E.
                    tx.queue(Segment::E);
                    tx.queue(Segment::Data);
                    self.stage = Stage::Data;
                }
                _ => {}
            }
        }
        if self.stage == (Stage::Retrain { initiator: false, answered: false })
            && self.modem.receiver.hearing() == Hearing::Ac
            && self.modem.receiver.hearing_for() > AC_SECONDS
        {
            // 12.8.2: PPh, then ALT for at least 16T.
            self.modem.transmitter.queue(Segment::Pph(self.reading));
            self.modem.transmitter.queue(Segment::Alt { at_least: ALT_LEAST });
            self.stage = Stage::Retrain { initiator: false, answered: true };
        }
        self.sync.extend(self.modem.receiver.take_sync_bits());
        self.data.extend(self.modem.receiver.take_bits());
        if self.stage == Stage::Mph && contains(&self.sync, &self.far_mph) {
            // 12.4.1.3: an MPh received while sending MPh; finish the one in
            // hand, send one E, then data.
            self.modem.transmitter.queue(Segment::E);
            self.modem.transmitter.queue(Segment::Data);
            self.stage = Stage::Data;
        }
        out
    }

    fn heard(&self, wanted: impl Fn(&Heard) -> bool) -> Vec<(u64, Heard)> {
        self.heard.iter().copied().filter(|(_, h)| wanted(h)).collect()
    }

    /// Turn the control channel off (12.6.3): 4T of scrambled ones, then
    /// silence.
    fn turn_off(&mut self) {
        self.modem.transmitter.queue(Segment::Ones(TURN_OFF_SYMBOLS));
    }

    /// Retrain the control channel (12.8), as the end that asks or the one
    /// that answers.
    fn retrain(&mut self, initiator: bool) {
        // An MPh read in the start-up before is no answer to this one.
        self.sync.clear();
        if initiator {
            self.modem.transmitter.queue(Segment::Ac);
        }
        self.stage = Stage::Retrain { initiator, answered: false };
    }

    /// Resynchronise (12.6.1.1): 70 ms of silence, Sh, S-bar-h, then ALT
    /// while listening for the far end's.
    fn resync(&mut self) {
        if self.source {
            let tx = &mut self.modem.transmitter;
            tx.queue(Segment::Silence(SILENCE_SYMBOLS));
            tx.queue(Segment::Sh(SH_SYMBOLS));
            tx.queue(Segment::ShBar(SH_BAR_SYMBOLS));
            tx.queue(Segment::Alt { at_least: ALT_LEAST });
        }
        self.stage = Stage::Resync;
    }
}

/// A two-wire line between a call end and an answer end.
#[derive(Debug)]
struct Line {
    call: End,
    answer: End,
    to_answer: VecDeque<f64>,
    to_call: VecDeque<f64>,
    /// Gains call to answer and answer to call.
    call_gain: f64,
    answer_gain: f64,
    /// How much of each end's own signal comes back into its receiver.
    echo: f64,
    noise: f64,
    seed: u32,
    up: dsp::Resampler,
    down: dsp::Resampler,
    into_answer: VecDeque<f64>,
    out_of_answer: VecDeque<f64>,
    buffer: Vec<f64>,
    last_call: f64,
    last_answer: f64,
    /// A stretch, in samples of the call end's clock, over which the call
    /// end sends a loud wideband signal as if a page were on the primary
    /// channel.
    page: Option<(usize, usize)>,
    n: usize,
}

/// How the line is.
#[derive(Debug, Clone, Copy)]
struct Conditions {
    one_way: f64,
    /// Loss each way, in decibels.
    call_loss: f64,
    answer_loss: f64,
    /// Each end's own signal back into its receiver, in decibels down.
    echo: f64,
    /// White noise at each receiver, as the signal to noise of the weaker
    /// direction in the voice band, 300 to 3400 Hz: the way a telephone
    /// line's is quoted. Against the 600 Hz a symbol's worth of noise
    /// occupies, a symbol sees 7.1 dB better.
    snr: f64,
    ppm: f64,
}

/// The voice band against the whole of what 16 kHz carries, in decibels.
const VOICE_BAND_DB: f64 = 4.12;

impl Conditions {
    fn clean() -> Self {
        Self { one_way: 0.010, call_loss: 10.0, answer_loss: 10.0, echo: f64::INFINITY, snr: 80.0, ppm: 0.0 }
    }

    /// A two-wire VoIP line: the answer end 20 dB quieter at the call end
    /// than the call end is at the answer end, each end's own signal 10 dB
    /// down in its own receiver -- 20 dB above the far end's at the call end
    /// -- noise, and the clocks 50 ppm apart.
    fn two_wire(snr: f64) -> Self {
        Self { one_way: 0.100, call_loss: 10.0, answer_loss: 30.0, echo: 10.0, snr, ppm: 50.0 }
    }

    /// The noise, in decibels under the nominal level, across the whole band.
    fn noise_db(&self) -> f64 {
        self.call_loss.max(self.answer_loss) + self.snr - VOICE_BAND_DB
    }
}

impl Line {
    fn new(call: End, answer: End, conditions: Conditions) -> Self {
        let delay = ((conditions.one_way * FS) as usize).max(1);
        let db = |x: f64| if x.is_finite() { 10f64.powf(-x / 20.0) } else { 0.0 };
        let ratio = 1.0 + conditions.ppm * 1e-6;
        Self {
            call,
            answer,
            to_answer: std::iter::repeat_n(0.0, delay).collect(),
            to_call: std::iter::repeat_n(0.0, delay).collect(),
            call_gain: db(conditions.call_loss),
            answer_gain: db(conditions.answer_loss),
            echo: db(conditions.echo),
            // Uniform noise of the right power: nominal is 0.707 rms.
            noise: db(conditions.noise_db()) * std::f64::consts::FRAC_1_SQRT_2 * 3f64.sqrt(),
            seed: 0x1234_5678,
            up: dsp::Resampler::new(FS, FS * ratio),
            down: dsp::Resampler::new(FS * ratio, FS),
            into_answer: VecDeque::new(),
            out_of_answer: VecDeque::new(),
            buffer: Vec::new(),
            last_call: 0.0,
            last_answer: 0.0,
            page: None,
            n: 0,
        }
    }

    fn uniform(&mut self) -> f64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        2.0 * (f64::from(self.seed) / f64::from(u32::MAX) - 0.5)
    }

    /// One sample of the call end's clock.
    fn step(&mut self) {
        let noise = self.noise * self.uniform();
        let heard_by_call = self.to_call.pop_front().unwrap_or(0.0) * self.answer_gain + self.echo * self.last_call + noise;
        let mut out = self.call.step(heard_by_call);
        if self.page.is_some_and(|(from, to)| (from..to).contains(&self.n)) {
            // A page on the primary channel: loud and all over the band.
            out += 0.7 * self.uniform();
        }
        self.last_call = out;
        self.to_answer.push_back(out);
        self.buffer.clear();
        let x = self.to_answer.pop_front().unwrap_or(0.0);
        self.up.process(x, &mut self.buffer);
        self.into_answer.extend(self.buffer.iter().copied());
        while let Some(x) = self.into_answer.pop_front() {
            let noise = self.noise * self.uniform();
            let out = self.answer.step(x * self.call_gain + self.echo * self.last_answer + noise);
            self.last_answer = out;
            self.buffer.clear();
            self.down.process(out, &mut self.buffer);
            self.out_of_answer.extend(self.buffer.iter().copied());
        }
        self.to_call.push_back(self.out_of_answer.pop_front().unwrap_or(0.0));
        self.n += 1;
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            self.step();
        }
    }

    /// Steps until `done` or `seconds`, whichever is first; true if `done`.
    fn run_until(&mut self, seconds: f64, done: impl Fn(&Self) -> bool) -> bool {
        let end = self.n + (seconds * FS) as usize;
        while self.n < end {
            self.step();
            if done(self) {
                return true;
            }
        }
        false
    }

    /// A jitter buffer's slip on the way to the answer end: the next 20 ms
    /// never arrive, or arrive twice.
    fn slip_towards_answer(&mut self, dropped: bool) {
        let n = (0.020 * FS) as usize;
        if dropped {
            self.to_answer.drain(..n);
        } else {
            let again: Vec<f64> = self.to_answer.iter().take(n).copied().collect();
            for x in again.into_iter().rev() {
                self.to_answer.push_front(x);
            }
        }
    }

    fn ends(&self) -> [&End; 2] {
        [&self.call, &self.answer]
    }
}

fn in_data(line: &Line) -> bool {
    line.ends().iter().all(|e| e.modem.receiver.phase() == Phase::Data && e.modem.transmitter.on_air() == Some(Kind::Data))
}

/// Both ends from silence to data, then `bits` each way; the line afterwards.
fn start_up_and_send(mut line: Line, bits: usize) -> Line {
    let started = line.run_until(3.0, in_data);
    for end in line.ends() {
        println!(
            "{:?}: {:?}, trained {:.1} dB, now {:.1} dB, drift {:.1} ppm, heard {:?}",
            end.modem.side(),
            end.modem.receiver.phase(),
            end.modem.receiver.trained_snr_db(),
            end.modem.receiver.snr_db(),
            end.modem.receiver.drift_ppm(),
            end.heard
        );
    }
    assert!(started, "never reached data");
    let rate = line.call.modem.transmitter.rate();
    for (end, seed) in [(&mut line.call, 7), (&mut line.answer, 11)] {
        end.modem.transmitter.send_bits(&pattern(bits, seed));
    }
    line.run(bits as f64 / f64::from(rate.bits_per_second()) + 0.5);
    line
}

/// Each end received the other's `bits`, whole.
fn check_data(line: &Line, bits: usize) {
    for (end, seed) in [(&line.answer, 7), (&line.call, 11)] {
        let sent = pattern(bits, seed);
        let got = &end.data;
        let errors = got.iter().zip(&sent).filter(|(a, b)| a != b).count();
        assert!(contains(got, &sent), "{:?} received {} bits, {errors} of the first wrong", end.modem.side(), got.len());
    }
}

/// Each end heard PPh in `reading`, trained on it, and saw E.
fn check_start_up(line: &Line, reading: Reading) {
    for end in line.ends() {
        let pph = end.heard(|h| matches!(h, Heard::Pph { .. }));
        assert_eq!(pph.len(), 1, "{:?}: {pph:?}", end.modem.side());
        assert!(matches!(pph[0].1, Heard::Pph { reading: r, .. } if r == reading), "{:?}", pph[0]);
        let trained = end.heard(|h| matches!(h, Heard::Trained { .. }));
        assert!(matches!(trained[..], [(_, Heard::Trained { on: Reference::Pph(r), .. })] if r == reading), "{trained:?}");
        assert_eq!(end.heard(|h| matches!(h, Heard::E { .. })).len(), 1);
        assert!(end.heard(|h| matches!(h, Heard::Lost { .. } | Heard::Untrained { .. } | Heard::Ac { .. } | Heard::Tone { .. })).is_empty());
        assert!(end.modem.receiver.is_locked());
        // ALT from a scrambler at zero reads 0 1 0 1 from its first bit, as
        // this modem sends it, and the far end's MPh came whole.
        let alt: Vec<bool> = (0..32).map(|i| i % 2 == 1).collect();
        assert_eq!(&end.sync[..32], &alt[..], "{:?}", end.modem.side());
        assert!(contains(&end.sync, &end.far_mph));
    }
}

#[test]
fn pph_is_a_perfect_sequence_as_i_reads_it_and_a_square_wave_as_printed() {
    for i in 0..PPH_SYMBOLS {
        for reading in [Reading::WithI, Reading::AsPrinted] {
            let z = reading.point(i);
            // Every symbol a diagonal point at unit magnitude, repeating
            // every eight.
            assert!((z.re.abs() - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
            assert!((z.im.abs() - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
            assert!((z - reading.point(i % PPH_PERIOD)).abs() < 1e-12);
        }
    }
    // In eighths of a turn: 1 1 3 1 1 5 3 5 with I, 1 1 1 1 5 5 5 5 printed.
    let eighths = |reading: Reading| -> Vec<i64> {
        (0..8).map(|i| (reading.point(i).arg() / (std::f64::consts::PI / 4.0)).round() as i64).map(|e| e.rem_euclid(8)).collect()
    };
    assert_eq!(eighths(Reading::WithI), vec![1, 1, 3, 1, 1, 5, 3, 5]);
    assert_eq!(eighths(Reading::AsPrinted), vec![1, 1, 1, 1, 5, 5, 5, 5]);
    let periodic = |reading: Reading, shift: usize| {
        let sum = (0..PPH_PERIOD).fold(Complex::ZERO, |s, i| s + reading.point(i) * reading.point(i + shift).conj());
        sum.abs() / PPH_PERIOD as f64
    };
    for shift in 1..PPH_PERIOD {
        assert!(periodic(Reading::WithI, shift) < 1e-12, "shift {shift}");
    }
    let printed: Vec<f64> = (1..PPH_PERIOD).map(|s| periodic(Reading::AsPrinted, s)).collect();
    for (s, expected) in printed.iter().zip([0.5, 0.0, 0.5, 1.0, 0.5, 0.0, 0.5]) {
        assert!((s - expected).abs() < 1e-12, "{printed:?}");
    }
}

#[test]
fn a_clean_line_starts_up_and_carries_data_both_ways_at_1200() {
    let line = Line::new(
        End::new(Side::Call, true, Rate::R1200, Reading::WithI),
        End::new(Side::Answer, false, Rate::R1200, Reading::WithI),
        Conditions::clean(),
    );
    let line = start_up_and_send(line, 1200);
    check_start_up(&line, Reading::WithI);
    check_data(&line, 1200);
}

#[test]
fn a_two_wire_line_carries_2400_both_ways_through_echo_delay_noise_and_a_clock_50_ppm_out() {
    // At the call end the far end is 30 dB down under this end's own signal
    // at 10 dB down, and the answer end's guard tone with it: this end's own
    // signal is 20 dB above the far end's in its own receiver.
    for snr in [30.0, 20.0] {
        let line = Line::new(
            End::new(Side::Call, true, Rate::R2400, Reading::WithI),
            End::new(Side::Answer, false, Rate::R2400, Reading::WithI),
            Conditions::two_wire(snr),
        );
        let line = start_up_and_send(line, 3000);
        check_start_up(&line, Reading::WithI);
        check_data(&line, 3000);
        // The weaker direction, into the call end, read within a few
        // decibels of what the noise in a symbol's 600 Hz allows.
        let snr_db = line.call.modem.receiver.snr_db();
        assert!(snr_db > snr + 7.1 - 3.0, "at {snr} dB: {snr_db:.1} dB");
        for end in line.ends() {
            let drift = end.modem.receiver.drift_ppm();
            assert!((drift.abs() - 50.0).abs() < 10.0, "{:?}: {drift:.1} ppm", end.modem.side());
        }
    }
}

#[test]
fn the_answer_modem_can_be_the_source_and_a_noisy_line_still_carries_1200() {
    // Ten decibels of signal to noise in the voice band at the call end, 17
    // in a symbol's band, which four points at 1200 bit/s shrug off.
    let conditions = Conditions { snr: 10.0, ..Conditions::two_wire(0.0) };
    let line = Line::new(
        End::new(Side::Call, false, Rate::R1200, Reading::WithI),
        End::new(Side::Answer, true, Rate::R1200, Reading::WithI),
        conditions,
    );
    let line = start_up_and_send(line, 1500);
    check_start_up(&line, Reading::WithI);
    check_data(&line, 1500);
}

#[test]
fn pph_as_printed_is_recognised_as_printed() {
    let line = Line::new(
        End::new(Side::Call, true, Rate::R1200, Reading::AsPrinted),
        End::new(Side::Answer, false, Rate::R1200, Reading::AsPrinted),
        Conditions::two_wire(30.0),
    );
    let line = start_up_and_send(line, 600);
    check_start_up(&line, Reading::AsPrinted);
    check_data(&line, 600);
    assert_eq!(line.answer.modem.receiver.reference(), Some(Reference::Pph(Reading::AsPrinted)));
}

#[test]
fn a_20_ms_slip_in_the_middle_of_data_costs_only_the_bits_it_took() {
    for dropped in [true, false] {
        for rate in [Rate::R1200, Rate::R2400] {
            let mut line = Line::new(
                End::new(Side::Call, true, rate, Reading::WithI),
                End::new(Side::Answer, false, rate, Reading::WithI),
                Conditions::two_wire(30.0),
            );
            assert!(line.run_until(3.0, in_data), "never reached data");
            // Three stretches of data, the slip in the middle one's time on
            // the line; the first and last come through whole.
            let bps = f64::from(rate.bits_per_second());
            let (first, middle, last) = (pattern(1200, 3), pattern(1200, 5), pattern(1200, 9));
            let tx = &mut line.call.modem.transmitter;
            for part in [&first, &middle, &last] {
                tx.send_bits(part);
            }
            line.run(0.1 + 1200.0 / bps + 600.0 / bps);
            line.slip_towards_answer(dropped);
            line.run(0.5 + 1800.0 / bps);
            let got = &line.answer.data;
            let receiver = &line.answer.modem.receiver;
            println!("{dropped} {rate:?}: {} bits, slips {}, {:?}", got.len(), receiver.slips(), line.answer.heard);
            assert!(contains(got, &first), "{dropped} {rate:?}: the data before the slip");
            assert!(contains(got, &last), "{dropped} {rate:?}: the data after the slip");
            // The slip did land in the middle stretch: it took bits out of
            // it or put some in twice. Twenty milliseconds are twelve whole
            // symbols at 600 baud and whole cycles of either carrier, so
            // nothing else is disturbed -- the receiver need not even find
            // the signal again.
            assert!(!contains(got, &middle), "{dropped} {rate:?}: the slip missed");
            let expected = 3600 + if dropped { -1 } else { 1 } * (0.020 * bps) as i64;
            let ours = got.windows(first.len()).position(|w| w == first).unwrap_or(0);
            let tail = got[ours..].windows(last.len()).position(|w| w == last).unwrap_or(0) as i64 + 1200;
            assert_eq!(tail, expected, "{dropped} {rate:?}: bits between the two ends");
            assert!(receiver.phase() == Phase::Data && receiver.carrier(), "{dropped} {rate:?}: {:?}", receiver.phase());
            assert!(line.answer.heard(|h| matches!(h, Heard::Lost { .. })).is_empty(), "{dropped} {rate:?}");
        }
    }
}

#[test]
fn a_resync_on_sh_after_a_page_brings_the_control_channel_back_at_its_rate() {
    for rate in [Rate::R1200, Rate::R2400] {
        let mut line = Line::new(
            End::new(Side::Call, true, rate, Reading::WithI),
            End::new(Side::Answer, false, rate, Reading::WithI),
            Conditions::two_wire(25.0),
        );
        assert!(line.run_until(3.0, in_data), "never reached data");
        // Both ends turn the control channel off (12.6.3), and the source
        // sends a page on the primary channel: 1.5 s of loud wideband
        // signal, which the recipient's control receiver hears and the
        // source's own hears come back.
        line.call.turn_off();
        line.answer.turn_off();
        line.run(0.1);
        line.page = Some((line.n, line.n + (1.5 * FS) as usize));
        line.run(1.6);
        for end in line.ends() {
            assert_eq!(end.modem.receiver.phase(), Phase::Hunting, "{:?} still reading a page as data", end.modem.side());
        }
        // 12.6.1.1 and 12.6.2.1: Sh and S-bar-h, ALT, E, and data again.
        line.call.resync();
        line.answer.resync();
        let before = [line.call.data.len(), line.answer.data.len()];
        line.call.modem.transmitter.send_bits(&pattern(1200, 21));
        line.answer.modem.transmitter.send_bits(&pattern(1200, 23));
        assert!(line.run_until(2.0, in_data), "{rate:?}: never back to data");
        line.run(0.3 + 1200.0 / f64::from(rate.bits_per_second()));
        for (end, seed, from) in [(&line.answer, 21, before[1]), (&line.call, 23, before[0])] {
            let side = end.modem.side();
            println!("{side:?} {rate:?}: {:?}", end.heard);
            assert!(contains(&end.data[from..], &pattern(1200, seed)), "{side:?} {rate:?}: the data after the resync");
            let order: Vec<&str> = end
                .heard
                .iter()
                .filter_map(|(_, h)| match h {
                    Heard::Sh { .. } => Some("Sh"),
                    Heard::Reversal { .. } => Some("reversal"),
                    Heard::Trained { on: Reference::Sh, .. } => Some("trained"),
                    Heard::E { .. } => Some("E"),
                    _ => None,
                })
                .collect();
            // Two Es: the start-up's and the resync's. The core is trained
            // when the window after S-bar-h is full, which is after E where
            // the far end's ALT was short and E fell inside it.
            assert_eq!(order[..3], ["E", "Sh", "reversal"], "{side:?} {rate:?}");
            let mut rest = order[3..].to_vec();
            rest.sort_unstable();
            assert_eq!(rest, ["E", "trained"], "{side:?} {rate:?}");
            assert_eq!(end.modem.receiver.reference(), Some(Reference::Sh));
        }
    }
}

#[test]
fn ac_is_heard_and_lasts_past_the_100_ms_12_8_2_waits_for() {
    let mut line = Line::new(
        End::new(Side::Call, true, Rate::R2400, Reading::WithI),
        End::new(Side::Answer, false, Rate::R2400, Reading::WithI),
        Conditions::two_wire(25.0),
    );
    assert!(line.run_until(3.0, in_data), "never reached data");
    // Either end asks for a retrain.
    line.answer.modem.transmitter.queue(Segment::Ac);
    let heard = |l: &Line| l.call.modem.receiver.hearing() == Hearing::Ac && l.call.modem.receiver.hearing_for() > AC_SECONDS;
    assert!(line.run_until(0.5, heard), "the call end never heard AC for 100 ms");
    let ac = line.call.heard(|h| matches!(h, Heard::Ac { .. }));
    assert_eq!(ac.len(), 1, "{:?}", line.call.heard);
    // And stopped reading AC as data.
    assert!(!line.call.heard(|h| matches!(h, Heard::Lost { .. })).is_empty());
    assert_eq!(line.call.modem.receiver.phase(), Phase::Hunting);
    // Heard soon after it reached the line: within 30 symbols.
    let Some(Sent::Began { at: sent, .. }) = std::iter::from_fn(|| line.answer.modem.transmitter.sent()).last() else {
        panic!("AC never went")
    };
    let Heard::Ac { at } = ac[0].1 else { unreachable!() };
    let late = at as f64 - sent as f64 - 0.100 * FS;
    assert!(late.abs() < 30.0 * FS / BAUD, "AC heard {late:.0} samples after the line's delay");
}

#[test]
fn a_retrain_on_ac_starts_again_from_data_and_changes_the_rates() {
    // 12.8, with PPh coming straight after data one way and after AC the
    // other rather than after silence; and at the end each direction at its
    // own rate, as MPh's bit 50 lets two modems ask.
    for initiator in [Side::Call, Side::Answer] {
        let mut line = Line::new(
            End::new(Side::Call, true, Rate::R1200, Reading::WithI),
            End::new(Side::Answer, false, Rate::R1200, Reading::WithI),
            Conditions::two_wire(25.0),
        );
        assert!(line.run_until(3.0, in_data), "never reached data");
        // Call to answer goes up to 2400; answer to call stays at 1200.
        line.call.modem.transmitter.set_rate(Rate::R2400);
        line.answer.modem.receiver.set_rate(Rate::R2400);
        line.call.retrain(initiator == Side::Call);
        line.answer.retrain(initiator == Side::Answer);
        let back = line.run_until(3.0, |l| l.ends().iter().all(|e| e.stage == Stage::Data) && in_data(l));
        for end in line.ends() {
            println!("{initiator:?} initiating, {:?}: {:?}", end.modem.side(), end.heard);
        }
        assert!(back, "{initiator:?} initiating: never back to data");
        // Queued only now, so that nothing of it can have gone before the
        // retrain.
        let before = [line.call.data.len(), line.answer.data.len()];
        line.call.modem.transmitter.send_bits(&pattern(2400, 41));
        line.answer.modem.transmitter.send_bits(&pattern(1200, 43));
        line.run(1.6);
        for (end, seed, bits, from) in [(&line.answer, 41, 2400, before[1]), (&line.call, 43, 1200, before[0])] {
            let side = end.modem.side();
            assert!(contains(&end.data[from..], &pattern(bits, seed)), "{side:?}: the data after the retrain");
            let pph = end.heard(|h| matches!(h, Heard::Pph { .. }));
            let trained = end.heard(|h| matches!(h, Heard::Trained { on: Reference::Pph(_), .. }));
            assert_eq!((pph.len(), trained.len()), (2, 2), "{side:?}: {:?}", end.heard);
        }
        let responder = if initiator == Side::Call { &line.answer } else { &line.call };
        assert_eq!(responder.heard(|h| matches!(h, Heard::Ac { .. })).len(), 1);
    }
}

/// A frequency shift, as an analogue line's carrier systems made one: the
/// signal and its Hilbert transform, turned by a phasor.
#[derive(Debug)]
struct Shift {
    taps: Vec<f64>,
    history: VecDeque<f64>,
    phase: f64,
    step: f64,
}

impl Shift {
    fn new(hz: f64) -> Self {
        let taps = (0..255)
            .map(|i| {
                let n = i as i64 - 127;
                let window = 0.54 - 0.46 * (std::f64::consts::TAU * i as f64 / 254.0).cos();
                if n % 2 == 0 { 0.0 } else { window * 2.0 / (std::f64::consts::PI * n as f64) }
            })
            .collect();
        Self { taps, history: std::iter::repeat_n(0.0, 255).collect(), phase: 0.0, step: std::f64::consts::TAU * hz / FS }
    }

    fn process(&mut self, x: f64) -> f64 {
        self.history.pop_front();
        self.history.push_back(x);
        let transformed: f64 = self.history.iter().zip(self.taps.iter().rev()).map(|(a, b)| a * b).sum();
        self.phase = (self.phase + self.step) % std::f64::consts::TAU;
        self.history[127] * self.phase.cos() - transformed * self.phase.sin()
    }
}

#[test]
fn a_carrier_7_hz_out_either_way_is_followed() {
    // Not a VoIP line's trouble, but an analogue one's: every carrier moved
    // 7 Hz, which the watch, the search for PPh, the decisions after it and
    // the core's carrier loop each have to follow.
    for hz in [-7.0, 7.0] {
        let mut line = Line::new(
            End::new(Side::Call, true, Rate::R2400, Reading::WithI),
            End::new(Side::Answer, false, Rate::R2400, Reading::WithI),
            Conditions::two_wire(25.0),
        );
        let (mut to_answer, mut to_call) = (Shift::new(hz), Shift::new(hz));
        let mut sent = false;
        while line.n < (4.5 * FS) as usize {
            if let Some(x) = line.to_answer.back_mut() {
                *x = to_answer.process(*x);
            }
            line.step();
            if let Some(x) = line.to_call.back_mut() {
                *x = to_call.process(*x);
            }
            if !sent && in_data(&line) {
                line.call.modem.transmitter.send_bits(&pattern(4800, 7));
                line.answer.modem.transmitter.send_bits(&pattern(4800, 11));
                sent = true;
            }
        }
        assert!(sent, "{hz} Hz: never reached data");
        check_start_up(&line, Reading::WithI);
        check_data(&line, 4800);
        for end in line.ends() {
            // The answer end's clock is 50 ppm fast, which moves the carriers
            // a tenth of a hertz more.
            let offset = end.modem.receiver.offset_hz();
            assert!((offset - hz).abs() < 0.3, "{:?} at {hz} Hz: {offset:.2} Hz", end.modem.side());
        }
    }
}

#[test]
fn a_tone_is_heard_as_a_tone() {
    // Tone A or B from phase 2's modulator, answered in 12.4.3.1 and 12.7:
    // a carrier with nothing on it, 20 dB down, with the answer modem's
    // guard tone under tone A.
    for side in [Side::Call, Side::Answer] {
        let mut rx = Receiver::new(side, FS);
        let mut tone = crate::v34::dpsk::Transmitter::new(far(side), FS);
        tone.send(&[false; 600]);
        for _ in 0..(0.8 * FS) as usize {
            rx.feed(0.1 * tone.next_sample());
        }
        assert_eq!(rx.hearing(), Hearing::Tone, "{side:?}");
        let heard: Vec<Heard> = std::iter::from_fn(|| rx.heard()).collect();
        assert!(matches!(heard[..], [Heard::Carrier { on: true, .. }, Heard::Tone { .. }]), "{side:?}: {heard:?}");
    }
}

#[test]
fn long_data_and_noise_are_never_taken_for_a_signal() {
    // Six seconds of data at 2400 bit/s each way on a poor line: nothing
    // after E but data -- no tone, AC, Sh, PPh or loss.
    let mut line = Line::new(
        End::new(Side::Call, true, Rate::R2400, Reading::WithI),
        End::new(Side::Answer, false, Rate::R2400, Reading::WithI),
        Conditions::two_wire(18.0),
    );
    assert!(line.run_until(3.0, in_data), "never reached data");
    let bits = 6 * 2400;
    line.call.modem.transmitter.send_bits(&pattern(bits, 31));
    line.answer.modem.transmitter.send_bits(&pattern(bits, 37));
    line.run(6.5);
    for end in line.ends() {
        let after_e = end.heard.iter().skip_while(|(_, h)| !matches!(h, Heard::E { .. })).skip(1).count();
        assert_eq!(after_e, 0, "{:?}: {:?}", end.modem.side(), end.heard);
        let hearing = end.modem.receiver.hearing();
        assert_eq!(hearing, Hearing::Modulated, "{:?}", end.modem.side());
    }
    // And noise alone, loud enough to be taken for a carrier, for as long.
    for side in [Side::Call, Side::Answer] {
        let mut rx = Receiver::new(side, FS);
        let mut seed = 77u32;
        for _ in 0..(6.0 * FS) as usize {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            rx.feed(0.05 * (f64::from(seed) / f64::from(u32::MAX) - 0.5));
        }
        let heard: Vec<Heard> = std::iter::from_fn(|| rx.heard()).collect();
        assert!(matches!(heard[..], [Heard::Carrier { on: true, .. }]), "{side:?}: {heard:?}");
    }
}
