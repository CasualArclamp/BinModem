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
//! Phase 4's exchange and the PCM upstream carrying bytes are V92-35/V92-39/
//! V92-41: what is here stops at E1u on one side and TRN2d on the other.

use datapump::v90::sequences::{Cp, Descriptor};
use datapump::v90::ucode::Law;
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
    digital_notes: Vec<String>,
    analogue_notes: Vec<String>,
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
        // A line's passband, 31 taps of Hamming-windowed sinc at 8 kHz.
        //
        // A telephone line's passband, which is where Phase 3's timing problem
        // comes from: a line this narrow leaves Su nearly a single tone at
        // 1333 Hz, so anything that is to see a reversal has to see it *as a
        // 180 degree turn and a timing step together* rather than as a change
        // of waveform (`v92::receiver`'s `turned_share`).
        const CUTOFF: f64 = 3400.0;
        const HALF: i64 = 15;
        let mut taps: Vec<f64> = (-HALF..=HALF)
            .map(|k| {
                let t = k as f64;
                let sinc = if t.abs() < 1e-12 {
                    2.0 * CUTOFF / 8000.0
                } else {
                    (std::f64::consts::PI * t * 2.0 * CUTOFF / 8000.0).sin()
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
            digital_notes: Vec::new(),
            analogue_notes: Vec::new(),
        }
    }

    /// One tick: the analogue's previous symbol goes to the digital end, what the
    /// digital end sends comes back. That one-sample delay is the line, and it is
    /// what keeps the two of them causally apart.
    fn tick(&mut self) {
        let down = self.digital.step(self.up);
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
        self.up = self.line.step(sent);
        self.digital_notes.append(&mut self.digital.take_notes());
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
                && call.analogue.status() == analogue::Status::Phase3Done
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

/// The same call with an MD in it: this end announces 3 x 276 = 828 symbols in
/// INFO1a, sends it between the two Ru reversals, and the digital end waits
/// exactly that long before it hunts Ru again (9.5.1.1.1).
#[test]
fn phase_3_completes_with_md() {
    let mut call = Call::new(digital_settings(3), analogue_settings(3));
    let reached = call.run_until(
        |call| {
            call.digital.status() == digital::Status::Phase4
                && call.analogue.status() == analogue::Status::Phase3Done
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
                    && call.analogue.status() == analogue::Status::Phase3Done
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
                && call.analogue.status() == analogue::Status::Phase3Done
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
