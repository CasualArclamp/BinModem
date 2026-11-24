//! The start of an HST call: who speaks first, and when the session begins.
//!
//! The Courier's supervisor starts an HST answer by having the DSP send the
//! 2100 Hz answer tone and then start the forward transmitter with the back
//! receiver (DSP 0xC29D); the caller listens for that tone and, once it has
//! stopped, starts the back transmitter with the forward receiver (0xC573).
//! No V.8 and no phase reversals: a plain V.25 answer tone, which is also
//! what lets a V.22bis or V.32 caller fall in behind it on a dual-standard
//! modem.
//!
//! The lengths here are V.25's and not measured from a Courier, whose own
//! timers are known only in units the firmware does not name (see the notes
//! in `docs/design/hst.md`).

use super::session::{Config, Event, Session, Side};
use v8::AnswerTone;

/// V.25: 1.8 to 2.5 s of silence after going off hook, for the billing delay.
const BILLING_DELAY_MS: u32 = 2000;
/// V.25: 2.6 to 4.0 s of answer tone.
const ANSWER_TONE_MS: u32 = 3300;
/// V.25: 75 ms of silence between the answer tone and the data carrier.
const GAP_MS: u32 = 75;
/// The answer tone a caller hears must last at least this long before it is
/// taken for one, so a burst of something else does not start the session.
const TONE_HEARD_MS: u32 = 400;
/// And must have been gone this long before the caller takes it as over.
const TONE_GONE_MS: u32 = 30;
/// Peak level of the answer tone, on the same scale as the data carriers.
const TONE_LEVEL: f64 = 0.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Answerer: quiet for the billing delay.
    Billing,
    /// Answerer: the 2100 Hz tone.
    Tone,
    /// Answerer: the gap after it.
    Gap,
    /// Caller: listening for the tone, then for its end.
    Listening,
    /// Both: the session has the line.
    Running,
}

/// One end of an HST call from the moment it goes off hook.
#[derive(Debug)]
pub struct Call {
    fs: f64,
    side: Side,
    cfg: Config,
    phase: Phase,
    /// Samples spent in the current phase.
    n: u64,
    ear: AnswerTone,
    heard: u64,
    gone: u64,
    osc: f64,
    session: Option<Session>,
}

impl Call {
    pub fn new(fs: f64, side: Side, cfg: Config) -> Self {
        Self {
            fs,
            side,
            cfg,
            phase: if side == Side::Answerer { Phase::Billing } else { Phase::Listening },
            n: 0,
            ear: AnswerTone::new(fs),
            heard: 0,
            gone: 0,
            osc: 0.0,
            session: None,
        }
    }

    fn ms(&self, ms: u32) -> u64 {
        (f64::from(ms) * self.fs / 1000.0).round() as u64
    }

    pub fn side(&self) -> Side {
        self.side
    }

    /// Whether the call is past its answer tone and the session is running.
    pub fn started(&self) -> bool {
        self.session.is_some()
    }

    /// What the call is doing, for a status line.
    pub fn phase_name(&self) -> &'static str {
        match self.phase {
            Phase::Billing => "billing delay",
            Phase::Tone => "answer tone",
            Phase::Gap => "after answer tone",
            Phase::Listening if self.heard > 0 => "hearing answer tone",
            Phase::Listening => "waiting for answer tone",
            Phase::Running => match &self.session {
                Some(s) if s.is_connected() => "data",
                _ => "MNP link request",
            },
        }
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    pub fn poll_event(&mut self) -> Option<Event> {
        self.session.as_mut().and_then(Session::poll_event)
    }

    fn begin(&mut self) {
        self.phase = Phase::Running;
        self.session = Some(Session::new(self.fs, self.side, self.cfg));
    }

    /// One sample in from the line, one out to it.
    pub fn step(&mut self, line: f64) -> f64 {
        self.n += 1;
        match self.phase {
            Phase::Billing => {
                if self.n >= self.ms(BILLING_DELAY_MS) {
                    self.phase = Phase::Tone;
                    self.n = 0;
                }
                0.0
            }
            Phase::Tone => {
                if self.n >= self.ms(ANSWER_TONE_MS) {
                    self.phase = Phase::Gap;
                    self.n = 0;
                    return 0.0;
                }
                self.osc = (self.osc + std::f64::consts::TAU * 2100.0 / self.fs) % std::f64::consts::TAU;
                TONE_LEVEL * self.osc.sin()
            }
            Phase::Gap => {
                if self.n >= self.ms(GAP_MS) {
                    self.begin();
                }
                0.0
            }
            Phase::Listening => {
                self.ear.feed(line);
                if self.ear.present() {
                    self.heard += 1;
                    self.gone = 0;
                } else if self.heard >= self.ms(TONE_HEARD_MS) {
                    self.gone += 1;
                    if self.gone >= self.ms(TONE_GONE_MS) {
                        self.begin();
                    }
                } else {
                    self.heard = 0;
                }
                0.0
            }
            Phase::Running => {
                let s = self.session.as_mut().expect("running without a session");
                s.feed(line);
                s.next_sample()
            }
        }
    }
}
