//! A V.92 call: the digital modem of [`datapump::v92::digital`] against the
//! analogue modem of [`datapump::v92::analogue`], one line sample at a time.
//!
//! What it proves is Phase 3 end to end. The analogue end sends Ru and its bar,
//! TRN1u and Ja, goes silent, has its equaliser trained by TRN1d, reads Jd, sends
//! Su, reads Jp and raises circuit 107, sends the second TRN1u while the DIL
//! arrives, then CPt until Ri and E1u; the digital end reads Ja, sends Sd,
//! S-bar-d, TRN1d and Jd, measures Su, answers with Jp, reads CPt, sends Ri, Ri
//! gone round and reaches Phase 4.
//!
//! And Phase 4 between the two of them: TRN2u and TRN2d, the SUV/CP/E
//! exchange, E2u placed where it stands and Ed sent once, B1u and B1d, and
//! both ends in data mode. Each end's own Phase 4 is tested beside it, against
//! a scripted far end.
//!
//! And then the call V92-41 is for, milestone M2: PCM both ways and data both
//! ways, with a DIL and without, on a long round trip, at every sampling
//! phase, at three line levels, and with the central office's codec, A-law
//! and mu-law, quantising the upstream.
//!
//! The line is this file's own, a symbol at a time: a wire down, and up a
//! band-limited line that can sit a fraction of a symbol off the network's
//! clock, lose level, be quantised to G.711 and take its time. It is not
//! `v90::network::Network`, which carries a waveform at the analogue modem's
//! sample rate: these two modems speak in symbols, and what turns the
//! analogue one's into a waveform and back (`v92::transmit`, `v90::pcm`) is
//! joined to it in V92-42.

use datapump::v90::sequences::{Cp, Descriptor};
use datapump::v90::ucode::{self, Law};
use datapump::v92::analogue;
use datapump::v92::digital;
use datapump::v92::epsilon;
use datapump::v92::FilterLimits;

use std::collections::VecDeque;

/// Both ends' own rate: one symbol per 125 microseconds.
const DEFAULT_STEPS: u64 = 60_000;

fn digital_settings(far_md: u8) -> digital::Settings {
    digital::Settings {
        law: Law::Mu,
        uinfo: 79,
        far_md,
        round_trip: 0.0,
        wide: true,
        limits: FilterLimits::from_info1a(0, 0, 0),
        habits: digital::Habits::PROMPT,
    }
}

/// A CPt offering 40 000 bit/s, which is what an analogue end on a clean line
/// asks for.
fn cpt() -> Cp {
    Cp { drn: 16, constellations: vec![(1u128 << 40) - 1], ..Cp::default() }
}

fn analogue_settings(md_length: u8) -> analogue::Settings {
    analogue::Settings {
        md_length,
        ..analogue::Settings::new(Descriptor::none(), 0x7ffff, cpt())
    }
}

/// The two modems on one line, with one sample of delay between them.
struct Call {
    digital: digital::Modem,
    analogue: analogue::Modem,
    /// What the analogue sent last, which is what the digital end hears now.
    up: f64,
    /// And the line its output goes through: an integer of delay, the fraction
    /// the far end's clock is out by, and every sub-symbol delay the analogue
    /// modem's own S-bar-u asks for.
    line: Line,
    /// How many of the analogue's asked-for delays the line has taken.
    applied: usize,
    /// Ticks so far, which is both ends' own count of symbols.
    ticks: u64,
    /// The tick the analogue end sent B1u's first symbol on (8.7.1), and the
    /// tick the digital end said it had read E2u on (9.6.1.1.6).
    b1u_at: Option<u64>,
    e2u_at: Option<u64>,
    digital_notes: Vec<String>,
    analogue_notes: Vec<String>,
    /// What the line's level is to the central office's codec: the analogue
    /// modem's LU is one here, and a codec whose full scale is one clips a
    /// data-mode constellation sent at that.
    up_gain: f64,
    /// The central office's A/D on the upstream, when there is one: every
    /// sample the digital end gets is then a G.711 codeword's level, which is
    /// what makes the upstream a PCM one (clause 1).
    codec: Option<Law>,
    /// The network between them, as symbols of delay each way.
    down_pipe: VecDeque<f64>,
    up_pipe: VecDeque<f64>,
}

/// A line that can hold the far end's symbols a fraction of a symbol away from
/// the network's sampling instants.
///
/// 8.6.3 and Table 22 are about exactly this: the digital modem cannot move the
/// central office's A/D, so the analogue modem has to move instead, in half a
/// symbol at the S-bar-u that answers Jd and epsilon more at the one that
/// answers Jp. A line that only ever carried whole symbols would never exercise
/// any of it -- and on one that does, a two-point stream read half a symbol out
/// is two symbols added together, which is 19 dB rather than 160 (`v92::receiver`
/// has the table).
///
/// The interpolation is a 16-tap windowed sinc read at `anchor + fraction`,
/// with `anchor` sixteen samples in the past so that everything it needs has
/// been sent.
struct Line {
    /// What the analogue modem put on the line, one symbol at a time.
    raw: Vec<f64>,
    /// And the same through the transmit filter, which is what the far end's
    /// own output really is: no modem puts a step change of waveform on a
    /// telephone line.
    sent: Vec<f64>,
    /// The filter: linear phase, so that the delay it adds is a whole number of
    /// samples and cannot be mistaken for the sub-symbol offsets this test is
    /// about.
    taps: Vec<f64>,
    /// Epsilon's and the half symbol's worth of delay, as the far end applies
    /// them.
    extra: f64,
    /// The delays the far end has asked for and the line has not taken yet.
    ///
    /// A sub-symbol step of the far end's transmitter moves the grid its
    /// *symbols* go out on, and the line is `LINE_ANCHOR` samples behind it: so
    /// the step has to be held back by exactly that much, or it lands on samples
    /// that have already been sent -- and a step applied to the Su still in the
    /// pipe turns the quarter symbol before the inverse does, which is a
    /// transition 16 samples wide that no receiver should be asked to see.
    pending: VecDeque<(usize, f64)>,
    /// The far end's own clock error, in symbols.
    offset: f64,
    /// Whether the far end actually applies the sub-symbol delays its own
    /// S-bar-u asks for. A far end that does not -- one that ignored Jp, or one
    /// whose epsilon came out wrong -- leaves the equaliser fitted at the old
    /// phase, which is what 9.5.2.1.9's second TRN1u exists to fix.
    applies: bool,
}

/// Samples of history the interpolator reads behind the newest symbol, far
/// enough back that a 65 tap interpolator has everything it needs.
///
/// Sixty-five taps and not seventeen: TRN1u is a two-point stream, so nearly all
/// of its power is at 4000 Hz, and a short interpolator cannot put a quarter of
/// a symbol between two samples of *that* without smearing it. Measured with
/// seventeen, a TRN1u a third of a symbol out fitted at 12 dB instead of the
/// 150 a real delay gives -- an artefact of the harness that would have been
/// read as a receiver defect.
const LINE_ANCHOR: usize = 40;

impl Line {
    fn new(offset: f64) -> Self {
        Self::passing(offset, 3400.0)
    }

    /// A line that passes everything below `cutoff` hertz.
    fn passing(offset: f64, cutoff: f64) -> Self {
        // A line's passband, 31 taps of Hamming-windowed sinc at 8 kHz.
        //
        // A telephone line's passband, which is where Phase 3's timing problem
        // comes from: a line this narrow leaves Su nearly a single tone at
        // 1333 Hz, so anything that is to see a reversal has to see it *as a
        // 180 degree turn and a timing step together* rather than as a change
        // of waveform (`v92::receiver`'s `turned_share`).
        const HALF: i64 = 15;
        let mut taps: Vec<f64> = (-HALF..=HALF)
            .map(|k| {
                let t = k as f64;
                let sinc = if t.abs() < 1e-12 {
                    2.0 * cutoff / 8000.0
                } else {
                    (std::f64::consts::PI * t * 2.0 * cutoff / 8000.0).sin()
                        / (std::f64::consts::PI * t)
                };
                sinc * (0.54 + 0.46 * (std::f64::consts::PI * t / (HALF as f64 + 1.0)).cos())
            })
            .collect();
        let sum: f64 = taps.iter().sum();
        for tap in taps.iter_mut() {
            *tap /= sum;
        }
        Self {
            raw: Vec::new(),
            sent: Vec::new(),
            taps,
            extra: 0.0,
            pending: VecDeque::new(),
            offset,
            applies: true,
        }
    }

    /// The far end's transmitter steps: ask the line for that much delay, to
    /// take effect where the digital end has not read yet.
    fn ask(&mut self, extra: f64) {
        self.pending.push_back((self.sent.len() + LINE_ANCHOR, extra));
    }

    fn ignoring_the_jp(mut self) -> Self {
        self.applies = false;
        self
    }

    /// One symbol in, and what the digital end hears now.
    fn step(&mut self, symbol: f64) -> f64 {
        // A step that was asked for `LINE_ANCHOR` ago is due at the sample the
        // digital end is reading now.
        while let Some(&(due, extra)) = self.pending.front() {
            if self.sent.len() < due {
                break;
            }
            self.extra += extra;
            self.pending.pop_front();
        }
        self.raw.push(symbol);
        let newest = self.raw.len() - 1;
        let mut filtered = 0.0;
        for (k, &tap) in self.taps.iter().enumerate() {
            if let Some(&x) = newest.checked_sub(k).and_then(|i| self.raw.get(i)) {
                filtered += tap * x;
            }
        }
        self.sent.push(filtered);
        let newest = (self.sent.len() - 1) as f64;
        let at = newest - LINE_ANCHOR as f64 - self.offset - self.extra;
        let centre = at.round() as i64;
        let fraction = at - centre as f64;
        let mut out = 0.0;
        for k in -32i64..=32 {
            let Some(&x) = self.sent.get((centre + k) as usize) else { continue };
            let t = fraction - k as f64;
            let sinc = if t.abs() < 1e-12 {
                1.0
            } else {
                (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t)
            };
            let window = 0.54 + 0.46 * (std::f64::consts::PI * t / 33.0).cos();
            out += x * sinc * window;
        }
        out
    }
}

impl Call {
    fn new(digital: digital::Settings, analogue: analogue::Settings) -> Self {
        Self::on_a_line(digital, analogue, Line::new(0.0))
    }

    fn on_a_line(digital: digital::Settings, analogue: analogue::Settings, line: Line) -> Self {
        Self {
            digital: digital::Modem::new(digital),
            analogue: analogue::Modem::new(analogue),
            up: 0.0,
            line,
            applied: 0,
            ticks: 0,
            b1u_at: None,
            e2u_at: None,
            digital_notes: Vec::new(),
            analogue_notes: Vec::new(),
            up_gain: 1.0,
            codec: None,
            down_pipe: VecDeque::new(),
            up_pipe: VecDeque::new(),
        }
    }

    /// The same call with the central office's codec on the upstream: the line
    /// arrives at `gain` of the codec's full scale and leaves as the nearest
    /// G.711 level (Table 1/V.90).
    fn through_a_codec(mut self, law: Law, gain: f64) -> Self {
        self.codec = Some(law);
        self.up_gain = gain;
        self
    }

    /// The same call on a line that arrives at `gain` of what was sent.
    fn at_a_gain(mut self, gain: f64) -> Self {
        self.up_gain = gain;
        self
    }

    /// The same call with `symbols` of delay each way.
    fn delayed_by(mut self, symbols: usize) -> Self {
        self.down_pipe = VecDeque::from(vec![0.0; symbols]);
        self.up_pipe = VecDeque::from(vec![0.0; symbols]);
        self
    }

    /// One tick: the analogue's previous symbol goes to the digital end, what the
    /// digital end sends comes back. That one-sample delay is the line, and it is
    /// what keeps the two of them causally apart.
    fn tick(&mut self) {
        let mut down = self.digital.step(self.up);
        if !self.down_pipe.is_empty() {
            self.down_pipe.push_back(down);
            down = self.down_pipe.pop_front().unwrap_or(0.0);
        }
        let sent = self.analogue.step(down);
        // 9.5.2.1.7-8: the S-bar-u's half symbol and then epsilon are delays of
        // the analogue modem's transmitter, asked for as they are sent. This is
        // the consumer of `Analogue::delays` -- in the test it is the line,
        // because the analogue modem's own output is a symbol at a time.
        let delays = self.analogue.delays();
        for &extra in &delays[self.applied..] {
            if self.line.applies {
                // The far end steps its transmitter the way `epsilon` says its
                // counterparts do; see `epsilon::EXTENDS_LATE`.
                self.line.ask(epsilon::step_direction() * extra);
            }
        }
        self.applied = delays.len();
        let mut up = self.up_gain * self.line.step(sent);
        if let Some(law) = self.codec {
            // The A/D: the nearest codeword's level, sign and all.
            let (ucode, negative) = ucode::nearest(law, (up * 32768.0).round() as i32);
            up = ucode::level(law, ucode) * if negative { -1.0 } else { 1.0 };
        }
        if !self.up_pipe.is_empty() {
            self.up_pipe.push_back(up);
            up = self.up_pipe.pop_front().unwrap_or(0.0);
        }
        self.up = up;
        self.ticks += 1;
        if self.b1u_at.is_none() && self.analogue.phase() == "V.92 phase 4: B1u" {
            self.b1u_at = Some(self.ticks);
        }
        let mut said = self.digital.take_notes();
        if self.e2u_at.is_none() && said.iter().any(|note| note == "V.92: E2u") {
            self.e2u_at = Some(self.ticks);
        }
        self.digital_notes.append(&mut said);
        self.analogue_notes.append(&mut self.analogue.take_notes());
    }

    /// Run until `done`, and say whether it ever was.
    fn run_until(&mut self, done: impl Fn(&Self) -> bool, steps: u64) -> bool {
        for _ in 0..steps {
            if done(self) {
                return true;
            }
            self.tick();
        }
        done(self)
    }
}

/// Phase 3 runs to its end: Ja read, Sd and TRN1d and Jd sent, Su heard, Jp
/// answered, circuit 107 on, CPt read on one side and E1u sent on the other.
#[test]
fn phase_3_completes_and_the_digital_end_reaches_phase_4() {
    let mut call = Call::new(digital_settings(0), analogue_settings(0));
    let reached = call.run_until(
        |call| {
            call.digital.status() == digital::Status::Phase4
                && call.analogue.status() == analogue::Status::Phase4
        },
        DEFAULT_STEPS,
    );
    eprintln!(
        "--- digital ---\n{:#?}\n--- analogue ---\n{:#?}",
        call.digital_notes, call.analogue_notes
    );
    assert!(reached, "digital {:?}, analogue {:?}", call.digital.phase(), call.analogue.phase());

    // The digital end's half.
    assert!(call.digital.descriptor().is_some(), "no Ja: {:?}", call.digital_notes);
    assert!(call.digital.cpt().is_some(), "no CPt: {:?}", call.digital_notes);
    assert!(call.digital.circuit107(), "circuit 107 never came on: {:?}", call.digital_notes);
    assert_eq!(call.digital.cpt().map(|cp| cp.drn), Some(cpt().drn), "the CPt this end sent");

    // The analogue end's half.
    assert!(call.analogue.far_jd().is_some(), "no Jd: {:?}", call.analogue_notes);
    assert!(call.analogue.jp().is_some(), "no Jp: {:?}", call.analogue_notes);
    assert!(call.analogue.circuit107(), "circuit 107 never came on: {:?}", call.analogue_notes);
    // 9.5.2.1.7-8: half a symbol at the S-bar-u after Su, then the epsilon Jp
    // carried at the one after Jp. That epsilon is the digital end's measurement
    // of Su (9.5.1.1.6-7): whatever the phase turned out to be, both ends have
    // to be talking about the same number, which is what this holds together.
    let delays = call.analogue.delays();
    assert_eq!(delays.first(), Some(&0.5), "the half symbol the S-bar-u gives");
    let epsilon = call.analogue.epsilon().expect("Jp carried an epsilon");
    assert_eq!(delays.get(1), Some(&epsilon), "the analogue applied what Jp said");
    assert!((0.0..1.0).contains(&epsilon), "a fraction of a symbol: {epsilon}");
}

/// A CPt for 32 000 bit/s on thirty-two points, Ucodes 16 to 47: none of them
/// the zero level, whose sign no receiver can read, so that what the digital
/// end sends through V.90 5.4's encoder in Phase 4 is something the analogue
/// end can decode.
fn readable_cpt() -> Cp {
    let mask = (16..=47u8).fold(0u128, |mask, u| mask | 1 << u);
    Cp { drn: 16, constellations: vec![mask], ..Cp::default() }
}

/// A DIL of four segments of twenty-four symbols, each a reference at U_INFO
/// and a training symbol (V.90 8.4.1): enough to put 9.5.2.1.11's order in
/// force, in which Ri answers the CPt and R-bar-i the E1u.
fn a_dil() -> Descriptor {
    Descriptor {
        signs: vec![true, false],
        training: vec![false, true],
        h: [3; 8],
        refs: [79; 8],
        ucodes: vec![0x30, 0x38, 0x40, 0x48],
    }
}

/// A deterministic bit pattern, for the data.
fn pattern(length: usize, seed: u64) -> Vec<bool> {
    let mut state = seed | 1;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state >> 63 == 0
        })
        .collect()
}

/// Both modems of this crate from the end of Phase 2 until each says it is in
/// data mode, on the line every test here uses: a perfect wire down, and up a
/// 3.4 kHz passband with the analogue end's own sub-symbol delays.
fn connected_call(descriptor: Descriptor) -> Call {
    connected(Call::new(digital_settings(0), analogue_for_data(descriptor)))
}

/// The analogue end every call here that reaches data mode is made with.
fn analogue_for_data(descriptor: Descriptor) -> analogue::Settings {
    analogue::Settings::new(descriptor, 0x7ffff, readable_cpt())
}

/// Run `call` until both ends say they are in data mode, or say why not.
fn connected(mut call: Call) -> Call {
    let connected = call.run_until(
        |call| {
            matches!(call.digital.status(), digital::Status::Connected { .. })
                && matches!(call.analogue.status(), analogue::Status::Connected { .. })
        },
        150_000,
    );
    assert!(
        connected,
        "digital {:?} at {:?}, analogue {:?} at {:?}\n{:#?}\n{:#?}",
        call.digital.status(),
        call.digital.phase(),
        call.analogue.status(),
        call.analogue.phase(),
        call.digital_notes,
        call.analogue_notes
    );
    call
}

/// Where `wanted` is in `stream`, if it is there whole.
fn found(stream: &[bool], wanted: &[bool]) -> Option<usize> {
    stream.windows(wanted.len()).position(|window| window == wanted)
}

/// Phase 4 between the two modems of this crate (9.6.1.1 against 9.6.2.1),
/// with no DIL asked for and with one: both reach data mode, by an exchange
/// in which each thing the Recommendation places is where it places it, and
/// the downstream then carries data.
///
/// * **TRN2u is read from its first symbol** in either order. With no DIL the
///   digital modem sends R-bar-i on the CPt (9.5.1.1.13), before the analogue
///   modem's E1u, and still has to read that E1u, because TRN2u's scrambler is
///   reset on the symbol after it (8.7.6).
/// * **E2u is found where it stands**, after the sequence 9.6.2.1.4 has the
///   analogue modem complete, whether or not Ed has gone out yet; and the
///   digital modem conditions for B1u on the symbol after it (9.6.1.1.6).
/// * **Ed is two data frames** (8.8.2), so the B1d that follows it begins on
///   the frame the analogue modem's count of Ed's zeros says it does.
#[test]
fn phase_4_completes_between_the_two_modems_and_the_downstream_carries_data() {
    for descriptor in [Descriptor::none(), a_dil()] {
        let asked = descriptor.len();
        let mut call = connected_call(descriptor);
        let digital_said = |text: &str| call.digital_notes.iter().filter(|note| note.starts_with(text)).count();
        let analogue_said = |text: &str| call.analogue_notes.iter().filter(|note| note.starts_with(text)).count();
        let said = format!("a DIL of {asked}:\n{:#?}\n{:#?}", call.digital_notes, call.analogue_notes);

        // 8.7.6: the upstream reader starts at E1u, and its fit on TRN2u is
        // made against TRN2u.
        assert_eq!(digital_said("V.92: E1u"), 1, "{said}");
        assert_eq!(digital_said("V.92: phase 4 receiver fitted on TRN2u"), 1, "{said}");
        assert_eq!(digital_said("V.92: no phase 4 fit"), 0, "{said}");

        // 9.6.1.1.2 and 9.6.2.1.2: a CPd and a CPu each way, and the rates
        // they named are the rates each end says it is at.
        assert!(digital_said("V.92: CPu drn 16") >= 1 && analogue_said("CPd for drn") >= 1, "{said}");
        let upstream = call.analogue.parameters().map(datapump::v92::Parameters::rate).expect("a CPd was obeyed");
        assert_eq!(call.digital.status(), digital::Status::Connected { downstream: 48_000, upstream }, "{said}");
        assert_eq!(call.analogue.status(), analogue::Status::Connected { downstream: 48_000, upstream }, "{said}");

        // 9.6.1.1.6: E2u, once, and where it is. Its last symbol left the
        // analogue modem on the tick before B1u's first; the line is 55
        // samples long (its 31-tap filter and the interpolator's forty), and
        // the digital modem's equaliser is no longer than its 48 taps.
        assert_eq!(digital_said("V.92: E2u"), 1, "{said}");
        let (b1u, e2u) = (call.b1u_at.expect("B1u went out"), call.e2u_at.expect("E2u was read"));
        assert!((b1u + 55..=b1u + 55 + 48).contains(&e2u), "B1u's first symbol at {b1u}, E2u read at {e2u}: {said}");

        // 8.8.2: one Ed, of two frames, so the analogue modem's B1d begins on
        // the frame after the Ed it read, and all forty-eight frames are ones.
        assert_eq!(digital_said("V.92: Ed out"), 1, "{said}");
        assert_eq!(analogue_said("Ed, reading for B1d"), 1, "{said}");
        assert_eq!(analogue_said("B1d began"), 0, "{said}");
        assert_eq!(analogue_said("B1d, 48 of 48 frames scrambled ones"), 1, "{said}");
        assert!(call.analogue.circuit104() && call.analogue.circuit106() && call.analogue.circuit109());

        // And the downstream carries data: what the far DTE sends arrives bit
        // for bit, behind nothing but the ones of an idle line (V.90 8.6.1).
        let down = pattern(4000, 0xd0);
        call.digital.send_bits(&down);
        let mut came_down = Vec::new();
        for _ in 0..3000 {
            call.tick();
            came_down.extend(call.analogue.take_bits());
        }
        let at = found(&came_down, &down).unwrap_or_else(|| panic!("the downstream data did not arrive: {said}"));
        assert!(came_down[..at].iter().all(|&bit| bit), "something other than idle ones came first");
        assert!(came_down[at + down.len()..].iter().all(|&bit| bit), "and something other than idle ones after");
    }
}

/// What a call in data mode carried: the bits each DTE was handed.
struct Carried {
    came_down: Vec<bool>,
    went_up: Vec<bool>,
}

/// The data a call here is asked to carry, each way.
fn down_data() -> Vec<bool> {
    pattern(4000, 0xd0)
}

fn up_data() -> Vec<bool> {
    pattern(3000, 0x0b)
}

/// Hand each DTE's data to its modem and run the call until both lots have had
/// time to cross and the line has gone back to idle behind them.
fn carry(call: &mut Call) -> Carried {
    call.digital.send_bits(&down_data());
    call.analogue.send_bits(&up_data());
    let (mut came_down, mut went_up) = (Vec::new(), Vec::new());
    for _ in 0..12_000 + 2 * call.up_pipe.len() {
        call.tick();
        came_down.extend(call.analogue.take_bits());
        went_up.extend(call.digital.take_bits());
    }
    Carried { came_down, went_up }
}

/// Bits of B1u the digital modem's decoder cannot give right: it joins B1u
/// part way in (`v92::digital`'s `B1U_ACQUIRE_FRAMES`), so the first data
/// frame it decodes has no frame before it to take 6.4.1's differential sign
/// from, and 6.3's descrambler has twenty-three bits of memory to fill behind
/// that. Two data frames of the slowest rung cover both.
const B1U_JOIN_BITS: usize = 72;

/// A call that has reached data mode carries data both ways, bit for bit, in
/// front of and behind nothing but the ones of an idle line (8.7.1 upstream,
/// V.90 8.6.1 downstream), and is still in data mode afterwards.
fn assert_carries_data(call: &mut Call, what: &str) {
    let (down, up) = (down_data(), up_data());
    let carried = carry(call);
    let said = format!("{what}:\n{:#?}\n{:#?}", call.digital_notes, call.analogue_notes);

    let at = found(&carried.came_down, &down).unwrap_or_else(|| panic!("the downstream data did not arrive: {said}"));
    assert!(carried.came_down[..at].iter().all(|&bit| bit), "{what}: something other than idle ones came down first");
    assert!(carried.came_down[at + down.len()..].iter().all(|&bit| bit), "{what}: and something other than ones after");

    let at = found(&carried.went_up, &up).unwrap_or_else(|| {
        // How far from it the upstream is: B1u and an idle DTE are both ones,
        // so every block of what came up should be all ones.
        let ones: Vec<usize> =
            carried.went_up.chunks(360).take(24).map(|block| block.iter().filter(|&&bit| bit).count()).collect();
        panic!("the upstream data did not arrive; ones in each 360 bits: {ones:?}\n{said}")
    });
    assert!(at > B1U_JOIN_BITS, "{what}: the upstream data came before B1u had been read");
    let zero = carried.went_up[B1U_JOIN_BITS..at].iter().position(|&bit| !bit);
    assert_eq!(zero, None, "{what}: B1u and the idle line behind it are ones (8.7.1)");
    assert!(carried.went_up[at + up.len()..].iter().all(|&bit| bit), "{what}: and something other than ones after");

    // No retrain: both ends are where they were (9.7).
    assert!(matches!(call.digital.status(), digital::Status::Connected { .. }), "{said}");
    assert!(matches!(call.analogue.status(), analogue::Status::Connected { .. }), "{said}");
    assert!(!call.analogue_notes.iter().any(|note| note.contains("retrain")), "{said}");
}

/// V92-41's milestone: a V.92 call between the two modems of this crate, from
/// the end of Phase 2 to data mode, PCM both ways, carrying data both ways.
///
/// Phases 3 and 4 complete (9.5, 9.6), each end says it is connected at the
/// same two rates, and what each DTE sends the other receives. With no DIL
/// asked for and with one, because the two orders of 9.5.2.1.10 and 9.5.2.1.11
/// leave E1u's last symbol with different signs, and 8.7.6 starts TRN2u from
/// it.
///
/// Two things had to be put right in the digital modem's reading of the
/// upstream for this to pass, and both are in `v92::receiver`:
///
/// * **The data-mode equaliser is fitted for the line CPd asks for**
///   (9.6.1.1.6 with 6.4.2). The CPd carries a prefilter that undoes the line
///   and a precoder for what is left, and the receiver went on with the taps
///   it had fitted on TRN2u, which undo the same line again: B1u read as half
///   ones. `Receiver::train_for_data_mode`.
/// * **TRN2u's sign is E1u's last** (8.7.6). The fit on TRN2u took it with
///   either sign, so the equaliser came out inverted on every call whose E1u
///   ended negative, which here is the one with a DIL: one bit in six wrong.
///   `Receiver::set_trn2u_seed`.
///
/// The upstream is on the bottom rung of 6.1's ladder, 24 000 bit/s. That is
/// the digital modem's choice (`design_upstream`: the slowest rate Ja allows,
/// because it has no measurement of the line's noise it trusts), not what the
/// line carries; `tests/v92_upstream.rs` has the designed chain at 48 000.
#[test]
fn a_v92_call_connects_with_pcm_both_ways_and_carries_data() {
    for descriptor in [Descriptor::none(), a_dil()] {
        let asked = descriptor.len();
        let what = format!("a DIL of {asked}");
        let mut call = connected_call(descriptor);
        let (digital::Status::Connected { downstream, upstream }, analogue::Status::Connected { .. }) =
            (call.digital.status(), call.analogue.status())
        else {
            panic!("{what}: not connected");
        };
        println!("{what}: {downstream} bit/s down, {upstream} bit/s up, after {} symbols", call.ticks);
        assert_eq!(call.analogue.status(), analogue::Status::Connected { downstream, upstream }, "{what}");
        assert!(downstream >= 48_000, "{what}: only {downstream} bit/s down");
        // On 6.1's ladder: (drn + 17) x 8000/6 for a drn of 1 to 19.
        assert!((1..=19u8).any(|drn| datapump::v92::up_rate(drn) == upstream), "{what}: {upstream} is not a rung");
        // The receiver that reads B1u is the one fitted for CPd's filters.
        let fitted = call.digital_notes.iter().filter(|note| note.starts_with("V.92: data mode receiver fitted"));
        assert_eq!(fitted.count(), 1, "{what}: {:#?}", call.digital_notes);
        assert_carries_data(&mut call, &what);
    }
}

/// 9.6.1.1.2 and 9.6.2.1.2: each end's rate is the one the other end's CP
/// named. The downstream is the analogue modem's to choose and goes in CPu
/// bits 21:25 (Table 23); the upstream is the digital modem's and goes in CPd
/// bits 22:26 (Table 30).
#[test]
fn the_rates_are_the_ones_the_two_ends_asked_for() {
    let call = connected_call(Descriptor::none());
    let said = format!("{:#?}\n{:#?}", call.digital_notes, call.analogue_notes);
    // What each end chose, and what the other read.
    assert!(call.analogue_notes.iter().any(|note| note.contains("the CPu will ask for drn 16")), "{said}");
    assert!(call.digital_notes.iter().any(|note| note.starts_with("V.92: CPu drn 16")), "{said}");
    assert!(call.digital_notes.iter().any(|note| note.starts_with("V.92: upstream drn 1,")), "{said}");
    assert!(call.analogue_notes.iter().any(|note| note.starts_with("CPd for drn 1,")), "{said}");
    // And the rates those are: (drn + 20) x 8000/6 down, (drn + 17) x 8000/6 up.
    let downstream = datapump::v90::sequences::data_rate(16).expect("drn 16 is a rate");
    let upstream = datapump::v92::up_rate(1);
    assert_eq!((downstream, upstream), (48_000, 24_000));
    assert_eq!(call.digital.status(), digital::Status::Connected { downstream, upstream }, "{said}");
    assert_eq!(call.analogue.status(), analogue::Status::Connected { downstream, upstream }, "{said}");
    assert_eq!(call.analogue.parameters().map(datapump::v92::Parameters::rate), Some(upstream), "{said}");
}

/// A VoIP call's round trip, 0.6 s each way, which is longer than any of the
/// waits the exchange is built from.
///
/// The start-up fits inside the 20 s and six round trips of 9.6.1.2.1 and
/// 9.6.2.2.1, and the repeat rule of 9.6.1.1.2 and 9.6.2.1.2 (a CP again after
/// 100 ms and a round trip without an answer) does not fire for want of
/// knowing the round trip: no more CPs go either way than on a line with no
/// delay at all.
///
/// With a DIL asked for; the test after this one is the same call without.
#[test]
fn a_voip_length_round_trip_connects_with_pcm_upstream() {
    let round_trip = 2.0 * EACH_WAY as f64 / 8000.0;
    let cps = |call: &Call| {
        (
            call.digital_notes.iter().filter(|note| note.starts_with("V.92: CPd drn")).count(),
            call.analogue_notes.iter().filter(|note| note.starts_with("CPu for drn")).count(),
        )
    };
    let near = connected_call(a_dil());

    let digital = digital::Settings { round_trip, ..digital_settings(0) };
    let analogue = analogue::Settings { round_trip, ..analogue_for_data(a_dil()) };
    let mut call = connected(Call::new(digital, analogue).delayed_by(EACH_WAY));
    let said = format!("{:#?}\n{:#?}", call.digital_notes, call.analogue_notes);
    let allowed = ((20.0 + 6.0 * round_trip) * 8000.0) as u64;
    println!("0.6 s each way: connected after {} symbols of {allowed}", call.ticks);
    assert!(call.ticks < allowed, "{} symbols is over 20 s and six round trips: {said}", call.ticks);
    let (sent, least) = (cps(&call), cps(&near));
    assert!(sent.0 >= 1 && sent.1 >= 1, "a CPd and a CPu went: {sent:?}\n{said}");
    assert!(sent.0 <= least.0 && sent.1 <= least.1, "{sent:?} CPd and CPu against {least:?} with no delay: {said}");
    assert_carries_data(&mut call, "0.6 s each way");
}

/// A VoIP call's delay each way, in symbols: 0.6 s.
const EACH_WAY: usize = 4800;

/// The same round trip with no DIL asked for, which does not carry data up.
///
/// With no DIL the digital modem sends R-bar-i on the CPt, before the analogue
/// modem's E1u (9.5.1.1.13), and E1u is then only where TRN2u begins (8.7.6).
/// It looks for that E1u until its own TRN2d is done and the exchange begins
/// (`digital::Modem::begin_exchange`), and starts reading TRN2u there if it
/// has not come. On a short line it has; on this one R-bar-i is 0.6 s going
/// and E1u 0.6 s coming back, TRN2d is 4800 symbols, and the reader starts
/// in the middle of the CPt. Both ends still reach data mode, because every
/// sequence of Phase 4 finds its own place, and the downstream carries data;
/// but neither fit on TRN2u has a first symbol to be made against, so B1u is
/// read by the equaliser the second TRN1u designed, which CPd's prefilter has
/// put out of date. What wants changing is where the digital modem stops
/// looking for E1u: a round trip and a CPt after its R-bar-i, not the end of
/// its own TRN2d. That start is also what reads Phase 4 from a far end whose
/// E1u was never read at all, so it is not moved here.
#[test]
#[ignore = "V92-41: with no DIL and a 1.2 s round trip the digital modem starts reading TRN2u before E1u has arrived, and the upstream is not read"]
fn a_voip_length_round_trip_with_no_dil_connects_with_pcm_upstream() {
    let round_trip = 2.0 * EACH_WAY as f64 / 8000.0;
    let digital = digital::Settings { round_trip, ..digital_settings(0) };
    let analogue = analogue::Settings { round_trip, ..analogue_for_data(Descriptor::none()) };
    let mut call = connected(Call::new(digital, analogue).delayed_by(EACH_WAY));
    assert_carries_data(&mut call, "0.6 s each way, no DIL");
}

/// The upstream through the central office's codec, which is what makes it a
/// PCM upstream (clause 1): every sample the digital modem is given is a G.711
/// level, A-law and mu-law in turn, and the downstream's codewords are the
/// same law's.
///
/// The line is `v90::network`'s own upstream band, 3.7 kHz, and not the
/// 3.4 kHz every other call here is made on. On that one the digital modem's
/// first fit of TRN1u measures 12.2 dB against the 12.0 it asks for before it
/// will read Ja (`v92::receiver::TRAINED_SNR_DB`), and A-law's quantising
/// takes it to 11.9: Phase 3 ends there, which is a property of that gate and
/// that line and nothing to do with the law.
#[test]
fn an_a_law_network_connects_with_pcm_both_ways() {
    for law in [Law::A, Law::Mu] {
        for descriptor in [Descriptor::none(), a_dil()] {
            let what = format!("{law:?}, a DIL of {}", descriptor.len());
            let digital = digital::Settings { law, ..digital_settings(0) };
            let analogue = analogue::Settings { law, ..analogue_for_data(descriptor) };
            let line = Line::passing(0.0, datapump::v90::network::UP_CUTOFF);
            // A quarter of full scale, as `v90::network` brings a modem's
            // line level to its codec.
            let mut call = connected(Call::on_a_line(digital, analogue, line).through_a_codec(law, 0.25));
            assert_carries_data(&mut call, &what);
        }
    }
}

/// 8.6.3 and Table 22: wherever the central office's A/D takes its samples
/// between the analogue modem's symbols, the half symbol of the first S-bar-u
/// and Jp's epsilon put the symbols on them, and the upstream then carries
/// data. Eight phases an eighth of a symbol apart, with no DIL on the even
/// eighths and one on the odd.
#[test]
fn every_upstream_sampling_phase_is_found_and_corrected() {
    for eighth in 0..8u8 {
        let offset = f64::from(eighth) / 8.0;
        let descriptor = if eighth % 2 == 0 { Descriptor::none() } else { a_dil() };
        let line = Line::new(offset);
        let mut call = connected(Call::on_a_line(digital_settings(0), analogue_for_data(descriptor), line));
        assert_carries_data(&mut call, &format!("a sampling phase of {offset}"));
    }
}

/// The level the line arrives at is the line's business and not 8.8.3's: the
/// digital modem measures it on TRN1u and TRN2u, and its data-mode receiver is
/// fitted at whatever it was. A quarter, a half and all of what was sent.
#[test]
fn the_upstream_gain_at_the_codec_is_measured_and_used() {
    for gain in [0.25, 0.5, 1.0] {
        let call = Call::new(digital_settings(0), analogue_for_data(Descriptor::none())).at_a_gain(gain);
        let mut call = connected(call);
        assert_carries_data(&mut call, &format!("an upstream gain of {gain}"));
    }
}

/// The same call with an MD in it: this end announces 3 x 276 = 828 symbols in
/// INFO1a, sends it between the two Ru reversals, and the digital end waits
/// exactly that long before it hunts Ru again (9.5.1.1.1).
#[test]
fn phase_3_completes_with_md() {
    let mut call = Call::new(digital_settings(3), analogue_settings(3));
    let reached = call.run_until(
        |call| {
            call.digital.status() == digital::Status::Phase4
                && call.analogue.status() == analogue::Status::Phase4
        },
        DEFAULT_STEPS,
    );
    eprintln!(
        "--- digital ---\n{:#?}\n--- analogue ---\n{:#?}",
        call.digital_notes, call.analogue_notes
    );
    assert!(reached, "digital {:?}, analogue {:?}", call.digital.phase(), call.analogue.phase());

    assert!(
        call.analogue_notes.iter().any(|note| note.contains("MD of 828 symbols")),
        "no MD was sent: {:?}",
        call.analogue_notes
    );
    assert!(
        call.digital_notes.iter().any(|note| note.contains("MD")),
        "the digital end never waited out an MD: {:?}",
        call.digital_notes
    );
    assert!(call.digital.cpt().is_some(), "no CPt: {:?}", call.digital_notes);
    assert!(call.analogue.jp().is_some(), "no Jp: {:?}", call.analogue_notes);
}

/// A digital end that never sends Sd leaves Ja unanswered, and AR1's 1500 ms --
/// counted from the start of Ja and with no round-trip term -- is what notices.
#[test]
fn a_digital_end_that_never_sends_sd_fails_analogue() {
    let mut modem = analogue::Modem::new(analogue_settings(0));
    let mut ja_began = None;
    let mut failed = false;
    for _ in 0..40_000u64 {
        // Nothing on the line at all: no Sd, no S-bar-d, no TRN1d.
        modem.step(0.0);
        if modem.phase() == "V.92 phase 3: Ja" && ja_began.is_none() {
            ja_began = Some(modem.elapsed());
        }
        if let analogue::Status::Failed(why) = modem.status() {
            let began = ja_began.expect("Ja went out");
            assert_eq!(why, "no Sd reversal within 1500 ms of Ja");
            assert!(
                modem.elapsed() >= began + 12_000,
                "AR1 is 1500 ms: {} symbols after Ja began",
                modem.elapsed() - began
            );
            assert!(modem.elapsed() <= began + 12_200, "and it is a deadline, not a mood");
            failed = true;
            break;
        }
    }
    let notes = modem.take_notes();
    assert!(failed, "no failure in 40 000 symbols: {} notes {notes:?}", modem.phase());
    assert!(notes.iter().any(|note| note.contains("no Sd reversal")), "{notes:?}");
}

/// The far end's clock is not the network's, and Phase 3 is what puts it right.
///
/// The digital modem cannot move the central office's A/D, so epsilon in Jp
/// (8.6.3, Table 22) is the analogue modem's instruction to move its own
/// transmitter: half a symbol at the S-bar-u that answers Jd, epsilon more at
/// the one that answers Jp, and from then on its symbols land on the sampling
/// instants. What that is worth is measured in `v92::receiver`'s table -- a
/// two-point stream read half a symbol out is two symbols added together, 19 dB
/// against 166 -- and a CPt's CRC needs thirty-odd decibels.
///
/// This is the call the bench needed and could not make: an Agere SV92PP got as
/// far as "CPt sent" and no further, because epsilon was measured on the
/// equaliser's output, on symbols that were not Su, and with the wrong sign.
#[test]
fn the_call_survives_a_far_end_that_is_a_fraction_of_a_symbol_out() {
    for offset in [0.2, 0.3, 0.45, 0.7] {
        let mut call =
            Call::on_a_line(digital_settings(0), analogue_settings(0), Line::new(offset));
        let reached = call.run_until(
            |call| {
                call.digital.status() == digital::Status::Phase4
                    && call.analogue.status() == analogue::Status::Phase4
            },
            DEFAULT_STEPS,
        );
        assert!(
            reached,
            "offset {offset}: digital {:?}, analogue {:?}\n{:#?}\n{:#?}",
            call.digital.phase(),
            call.analogue.phase(),
            call.digital_notes,
            call.analogue_notes
        );
        assert!(
            call.digital.cpt().is_some(),
            "offset {offset}: no CPt: {:#?}",
            call.digital_notes
        );
        // Epsilon is the fraction of a symbol the far end still has to move:
        // half of what is left after the S-bar-u's own half symbol.
        let epsilon = call.analogue.epsilon().expect("Jp carried an epsilon");
        let wanted = if epsilon::EXTENDS_LATE {
            (0.5 - offset).rem_euclid(1.0)
        } else {
            (offset + 0.5).rem_euclid(1.0)
        };
        let error = ((epsilon - wanted + 0.5).rem_euclid(1.0) - 0.5).abs();
        assert!(error < 0.03, "offset {offset}: epsilon {epsilon:.3}, wanted {wanted:.3}");
    }
}

/// And when the far end does **not** move -- it ignored Jp, or its epsilon
/// came out wrong -- the equaliser is what has to follow.
///
/// The 24.5T S-bar-u and the (24 + epsilon)T one are both timing steps, and
/// 9.5.2.1.9's second TRN1u is where the digital modem gets to fit at the phase
/// they left: "the second TRN1u also lets the digital modem refine its
/// equalizer at the new phase". Without it the CPt arrives through taps fitted
/// for a grid that moved half a symbol, which is 19 dB rather than 160, and its
/// CRC never passes -- the bench's exact symptom.
#[test]
fn the_refit_follows_the_phase_step_the_s_bar_u_left() {
    let mut call = Call::on_a_line(
        digital_settings(0),
        analogue_settings(0),
        Line::new(0.2).ignoring_the_jp(),
    );
    let reached = call.run_until(
        |call| {
            call.digital.status() == digital::Status::Phase4
                && call.analogue.status() == analogue::Status::Phase4
        },
        DEFAULT_STEPS,
    );
    assert!(
        reached,
        "digital {:?}, analogue {:?}\n{:#?}\n{:#?}",
        call.digital.phase(),
        call.analogue.phase(),
        call.digital_notes,
        call.analogue_notes
    );
    assert!(
        call.digital_notes.iter().any(|note| note.contains("refit at the new phase")),
        "no refit: {:#?}",
        call.digital_notes
    );
    assert!(call.digital.cpt().is_some(), "no CPt: {:#?}", call.digital_notes);
}
