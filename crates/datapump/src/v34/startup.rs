//! V.34 from the end of V.8 to the start of data: phase 2, and phases 3 and
//! 4 straight after it on what phase 2 settled.

use super::phase2::{self, Role};
use super::training::{self, Settings};
use dsp::{EchoCanceller, EchoFinder};

pub use dsp::Reflection;

/// How far back the first run of the canceller's taps looks, in
/// milliseconds: the reflection off a hybrid and a little behind it. The
/// same as V.32's canceller, which this one is a copy of in everything but
/// where its training segment is.
const ECHO_SPAN_MS: f64 = 8.0;

/// How much line the second run of taps covers, in milliseconds.
const FAR_SPAN_MS: f64 = 4.0;

/// The weakest reflection worth a second run of taps. See V.32's.
const FAINTEST: f64 = 0.15;

/// The canceller's step while it trains. See V.32's.
const ECHO_STEP: f64 = 0.25;

/// How much the canceller has to have taken out of what it heard, over its
/// training, to be kept: a decibel. A canceller trained on a line with no
/// echo has learned the line's noise and would add it back. See V.32's.
const NOTHING_CANCELLED_DB: f64 = 1.0;

/// How much of this end's TRN the far end's J can run into, over and above
/// a round trip, in seconds. See where it is used.
const STILL_J: f64 = 0.02;

/// How long the two cancellers are compared for at a time, in seconds.
const DRIFT_LOOK: f64 = 0.5;

/// How much more the taps that stay have to leave of the line than the
/// taps that follow, as a share of it, for the echo to have moved: an echo
/// that is back up to 25 dB below the far end.
const DRIFT_WORSE: f64 = 0.003;

/// The least distance back that a reflection is looked for, in seconds.
///
/// The round trip of phase 2 bounds a reflection off a network. A sound
/// card's cable is not a network: this end's signal comes back after the
/// card's own buffers, which the round trip to the far end says little
/// about. So the search goes this far in any case.
const LEAST_SEARCH: f64 = 0.12;

/// Retrains in a row that phase 2 may ask for before its failure is taken as
/// the end of the call.
///
/// Each one runs phase 2 again from its tones, and phase 2 gives up on its
/// own after twenty seconds, so this bounds how long a far end that has gone
/// can keep this end busy.
const PHASE2_RETRAINS: u32 = 2;

/// How the start-up is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// Phase 4 is over but the two MPs left no data mode to run.
    Done,
    /// In data mode, at these rates in bit/s.
    Connected { transmit: u32, receive: u32 },
    /// Back from data mode at MP, for a rate renegotiation or a cleardown.
    Retraining,
    /// A cleardown has ended the call.
    ClearedDown,
    Failed(&'static str),
}

/// One end of the start-up.
#[derive(Debug, Clone)]
pub struct Modem {
    fs: f64,
    phase2: phase2::Modem,
    training: Option<training::Modem>,
    /// Set while a retrain is under way: phase 2 is running again after a call
    /// was up, so the call is recovering rather than being placed. Cleared
    /// when the new phases 3 and 4 connect.
    retraining: bool,
    /// Full retrains since the call began (11.5), as against the in-band rate
    /// renegotiations [`training::Modem`] counts.
    retrains: u32,
    /// Retrains phase 2 has asked for since phases 3 and 4 last connected.
    phase2_retrains: u32,
    /// Whether the phases 3 and 4 now running have reached phase 4, and data
    /// mode.
    reached_phase4: bool,
    reached_data: bool,
    /// Both directions of V.34 are in one band. On a two-wire line this
    /// end's own signal comes back, and the canceller takes it out.
    echo: EchoCanceller,
    /// Looks for a reflection that the first run of taps does not reach,
    /// in the first half of this end's TRN.
    finder: Option<EchoFinder>,
    /// Samples of this end's TRN to let go by before the search starts:
    /// the far end's J can still be arriving. See [`STILL_J`].
    search_in: usize,
    searched: usize,
    search_for: usize,
    reflection: Option<Reflection>,
    /// The sample this end sent last.
    sent: f64,
    was_training: bool,
    /// What arrived and what the canceller left, over this end's TRN.
    heard_in_training: f64,
    left_in_training: f64,
    trained_loss: f64,
    /// What went out and what came back over this end's TRN, for
    /// [`EchoCanceller::fit`], with enough of what went out before it to lie
    /// under the furthest tap.
    recent: std::collections::VecDeque<(f64, f64)>,
    recorded: Option<(Vec<f64>, Vec<f64>)>,
    /// Samples of the recording that are from before the TRN.
    recorded_before: usize,
    /// How far down the two sets of taps left the recording of the last
    /// TRN: the taps as they were learned, then as they were fitted.
    fitted: Option<(f64, f64)>,
    /// A copy of the canceller that follows drift. See [`Self::watch_drift`].
    follower: Option<EchoCanceller>,
    following: bool,
    /// What each of the two left of the line, over the look now running.
    left: (f64, f64),
    left_for: usize,
    worse_looks: u32,
    trace: (f64, f64, f64, usize),
}

impl Modem {
    pub fn new(role: Role, fs: f64) -> Self {
        Self::with_phase2(phase2::Modem::new(role, fs), fs)
    }

    /// A start-up whose phase 2 is `phase2` -- V.90's, say, which comes to
    /// V.34's phases 3 and 4 when the far end turns out not to be a V.90
    /// digital modem.
    pub fn with_phase2(phase2: phase2::Modem, fs: f64) -> Self {
        Self {
            fs,
            phase2,
            training: None,
            retraining: false,
            retrains: 0,
            phase2_retrains: 0,
            reached_phase4: false,
            reached_data: false,
            echo: EchoCanceller::new((ECHO_SPAN_MS * fs / 1000.0) as usize, ECHO_STEP),
            finder: None,
            search_in: 0,
            searched: 0,
            search_for: 0,
            reflection: None,
            sent: 0.0,
            was_training: false,
            heard_in_training: 0.0,
            left_in_training: 0.0,
            trained_loss: 0.0,
            recent: std::collections::VecDeque::new(),
            recorded: None,
            recorded_before: 0,
            fitted: None,
            follower: None,
            following: false,
            left: (0.0, 0.0),
            left_for: 0,
            worse_looks: 0,
            trace: (0.0, 0.0, 0.0, 0),
        }
    }

    /// How far down the canceller's taps left the recording of this end's
    /// last TRN, in decibels: as they were learned, then as they were
    /// fitted. `None` if there was no fit.
    pub fn echo_fit(&self) -> Option<(f64, f64)> {
        self.fitted
    }

    /// How much of its own echo the canceller had taken out when this end's
    /// TRN ended, in decibels. That is the last moment the number means
    /// anything: after it the far end is in what arrives and in what is left.
    pub fn echo_return_loss(&self) -> f64 {
        self.trained_loss
    }

    /// What the canceller is taking out at this instant.
    pub fn echo_return_loss_now(&self) -> f64 {
        self.echo.echo_return_loss()
    }

    /// The reflection the canceller found past its first run of taps.
    pub fn reflection(&self) -> Option<Reflection> {
        self.reflection
    }

    /// Take this end's own echo out of a sample from the line.
    ///
    /// The canceller learns only while this end sends the TRN of phase 3,
    /// when the far end is silent. The first half of that TRN finds where a
    /// far reflection is, and the second half fits it.
    fn cancel(&mut self, line: f64) -> f64 {
        let training = self.training.as_ref().is_some_and(training::Modem::training_echo);
        if training && !self.was_training {
            self.begin_search();
            (self.heard_in_training, self.left_in_training) = (0.0, 0.0);
            // The recording starts with what is already under the taps,
            // and what came back with it.
            self.recorded = Some(self.recent.iter().copied().unzip());
            self.recorded_before = self.recent.len();
        }
        // What went out lately, as far back as a tap can be placed.
        let keep = self.search_reach() + self.echo.span() + 1;
        self.recent.push_back((self.sent, line));
        while self.recent.len() > keep {
            self.recent.pop_front();
        }
        if let (true, Some((sent, heard))) = (training, self.recorded.as_mut()) {
            sent.push(self.sent);
            heard.push(line);
        }
        self.echo.set_adapting(training);
        let cleaned = self.echo.process(self.sent, line);
        if self.search_in > 0 {
            self.search_in -= 1;
            // Not scored, but what goes out now is what the echo scored
            // later is an echo of.
            if let Some(finder) = self.finder.as_mut() {
                finder.remember(self.sent);
            }
        } else if let Some(finder) = self.finder.as_mut() {
            // What the near taps left, and not what arrived: see V.32's.
            finder.feed(self.sent, cleaned);
            self.searched += 1;
            if self.searched >= self.search_for {
                self.place_far_taps();
            }
        }
        self.watch_drift(line, cleaned);
        if training && self.finder.is_none() {
            self.heard_in_training += line * line;
            self.left_in_training += cleaned * cleaned;
        }
        if self.was_training && !training {
            self.finder = None;
            self.trained_loss = self.echo.echo_return_loss();
            let removed = 10.0 * (self.heard_in_training / self.left_in_training.max(1e-30)).log10();
            let recorded = self.recorded.take();
            if self.heard_in_training <= 1e-12 || removed <= NOTHING_CANCELLED_DB {
                // No echo on this line. What the taps learned is noise.
                self.echo.reset();
                self.reflection = None;
                self.fitted = None;
            } else if let Some((sent, heard)) = recorded {
                // The whole TRN, solved outright. What came back before
                // the TRN is not fitted: the far end was not silent then.
                //
                // Nor is the start of the TRN. The answer modem stops its J
                // when it hears the S of the call modem, and that silence
                // is a round trip and the time to detect S behind the S
                // itself. S, S-bar and PP are 432 symbols, which a long
                // round trip uses up; the J then runs into the TRN.
                let late = self.still_j().min((sent.len() - self.recorded_before) / 2);
                self.fitted = self.echo.fit(&sent, &heard, self.recorded_before + late);
                if let Some((learned, fitted)) = self.fitted {
                    self.trained_loss = self.trained_loss.max(learned.max(fitted));
                }
            }
            if std::env::var_os("V34_ECHO_TRACE").is_some() {
                eprintln!(
                    "v34 echo[{:?}]: round trip {:?} s, removed {removed:.1} dB, reflection {:?}, fit {:?}",
                    self.phase2.role(),
                    self.phase2.round_trip(),
                    self.reflection,
                    self.fitted
                );
            }
            // A sound card's cable does not keep its echo where training
            // found it. See [`Self::watch_drift`].
            //
            // A reflection off the far end of a network does not drift by
            // clocks, and it can go away while the far end talks: a network
            // echo canceller, as on NIST's line. As V.32 does: follow only
            // its jumps, hold its rate at nothing, and scale the far taps to
            // the echo that is still there (`EchoCanceller::hold_drift_rate`).
            let network = (training::NETWORK_ECHO * self.fs) as usize;
            if self.echo.far_echo().is_some_and(|(offset, _)| offset >= network) {
                self.echo.follow_drift(true);
                self.echo.hold_drift_rate();
                self.follower = None;
            } else if self.following {
                self.echo.follow_drift(true);
            } else if self.fitted.is_some() {
                let mut follower = self.echo.clone();
                follower.follow_drift(true);
                self.follower = Some(follower);
                (self.left, self.left_for, self.worse_looks) = ((0.0, 0.0), 0, 0);
            }
        }
        self.was_training = training;
        if std::env::var_os("V34_ECHO_TRACE").is_some() {
            self.trace.0 += line * line;
            self.trace.1 += cleaned * cleaned;
            self.trace.2 += self.sent * self.sent;
            self.trace.3 += 1;
            if self.trace.3 >= (0.1 * self.fs) as usize {
                let db = |x: f64| 10.0 * x.max(1e-30).log10();
                eprintln!(
                    "v34 trace[{:?}] {}: snr {:.1} line {:.1} left {:.1} sent {:.1} dB",
                    self.phase2.role(),
                    self.phase(),
                    self.snr_db().unwrap_or(0.0),
                    db(self.trace.0 / self.trace.3 as f64),
                    db(self.trace.1 / self.trace.3 as f64),
                    db(self.trace.2 / self.trace.3 as f64)
                );
                self.trace = (0.0, 0.0, 0.0, 0);
            }
        }
        cleaned
    }

    fn begin_search(&mut self) {
        let Some(training) = self.training.as_ref() else {
            return;
        };
        let first = self.echo.span();
        let last = self.search_reach();
        if last <= first {
            return;
        }
        let all = (training.training_echo_seconds() * self.fs) as usize;
        self.finder = Some(EchoFinder::new(first, last));
        self.searched = 0;
        if self.phase2.round_trip().unwrap_or(0.0) >= training::NETWORK_ECHO {
            // On a network line the echo of TRN is a round trip behind its
            // start. The search scores from then on, for half of what is
            // left; the other half is for the far taps to learn. The far
            // taps read what went out a round trip before they are placed,
            // so the canceller keeps that much.
            self.echo.remember(last);
            self.search_in = self.still_j().min(all / 2);
            self.search_for = (all - self.search_in) / 2;
        } else {
            self.search_in = self.still_j().min(all / 4);
            self.search_for = all / 2 - self.search_in;
        }
    }

    fn place_far_taps(&mut self) {
        let Some(finder) = self.finder.take() else {
            return;
        };
        let Some(found) = finder.best().filter(|f| f.strength >= FAINTEST) else {
            return;
        };
        let taps = self.far_taps();
        let offset = found.delay.saturating_sub(taps / 2).max(self.echo.span());
        self.echo.watch_far_echo(offset, taps);
        self.reflection = Some(found);
    }

    /// How many samples of this end's TRN the far end's J can run into.
    fn still_j(&self) -> usize {
        ((self.phase2.round_trip().unwrap_or(0.0) + STILL_J) * self.fs) as usize
    }

    /// Change to the canceller that follows drift, if the echo has moved.
    ///
    /// Taps that stay where training put them cancel an echo that stays
    /// where it was almost exactly: 53 dB of signal to noise in phase 4
    /// against an echo as loud as the far end, and 33 600 bit/s. Taps that
    /// follow drift read between samples, and that costs them: 35 to 40 dB
    /// on the same line, and 24 000 bit/s. But on a sound card's cable with
    /// two clocks the echo does move, and taps that stay are then useless
    /// in seconds. So both run. The second is a copy that follows drift,
    /// and it takes over when it leaves less of the line than the first.
    ///
    /// The two hear the same far end, so the difference between what they
    /// leave is the difference between their echoes and little else.
    fn watch_drift(&mut self, line: f64, cleaned: f64) {
        let Some(follower) = self.follower.as_mut() else {
            return;
        };
        let followed = follower.process(self.sent, line);
        self.left.0 += cleaned * cleaned;
        self.left.1 += followed * followed;
        self.left_for += 1;
        if self.left_for < (DRIFT_LOOK * self.fs) as usize {
            return;
        }
        let worse = self.left.0 > self.left.1 * (1.0 + DRIFT_WORSE);
        self.worse_looks = if worse { self.worse_looks + 1 } else { 0 };
        (self.left, self.left_for) = ((0.0, 0.0), 0);
        if self.worse_looks >= 2
            && let Some(follower) = self.follower.take()
        {
            self.echo = follower;
            self.following = true;
            if std::env::var_os("V34_ECHO_TRACE").is_some() {
                eprintln!("v34 echo[{:?}]: the echo moves; following it", self.phase2.role());
            }
        }
    }

    /// Whether the canceller has changed to the taps that follow drift.
    pub fn echo_following(&self) -> bool {
        self.following
    }

    /// The furthest back a reflection is looked for, in samples.
    fn search_reach(&self) -> usize {
        let round_trip = self.phase2.round_trip().unwrap_or(0.0);
        (round_trip.max(LEAST_SEARCH) * self.fs) as usize + self.far_taps()
    }

    fn far_taps(&self) -> usize {
        (FAR_SPAN_MS * self.fs / 1000.0) as usize
    }

    /// Full retrains since the call began.
    pub fn retrains(&self) -> u32 {
        self.retrains
    }

    pub fn role(&self) -> Role {
        self.phase2.role()
    }

    pub fn phase2(&self) -> &phase2::Modem {
        &self.phase2
    }

    /// Phase 2, to be told something before it runs.
    pub fn phase2_mut(&mut self) -> &mut phase2::Modem {
        &mut self.phase2
    }

    /// Phases 3 and 4, once phase 2 is over.
    pub fn training(&self) -> Option<&training::Modem> {
        self.training.as_ref()
    }

    pub fn status(&self) -> Status {
        match (self.phase2.status(), self.training.as_ref().map(training::Modem::status)) {
            (phase2::Status::Failed(why), _) => Status::Failed(why),
            (_, Some(training::Status::Failed(why))) => Status::Failed(why),
            (_, Some(training::Status::Done)) => Status::Done,
            (_, Some(training::Status::Connected { transmit, receive })) => Status::Connected { transmit, receive },
            (_, Some(training::Status::Retraining)) => Status::Retraining,
            (_, Some(training::Status::ClearedDown)) => Status::ClearedDown,
            // Phase 2 running again after a call was up is a retrain, not a
            // call being placed: the call is up as far as anything above is
            // concerned, and recovering.
            _ if self.retraining => Status::Retraining,
            _ => Status::Running,
        }
    }

    /// Begin a full retrain from data mode (11.5): go back through phase 2 and
    /// train again. False, and nothing done, outside data mode.
    pub fn retrain(&mut self) -> bool {
        self.training.as_mut().is_some_and(training::Modem::start_retrain)
    }

    /// Data received.
    pub fn take_bits(&mut self) -> Vec<bool> {
        self.training.as_mut().map(training::Modem::take_bits).unwrap_or_default()
    }

    /// Data to send, once in data mode.
    pub fn send_bits(&mut self, bits: &[bool]) {
        if let Some(training) = self.training.as_mut() {
            training.send_bits(bits);
        }
    }

    pub fn pending_bits(&self) -> usize {
        self.training.as_ref().map_or(0, training::Modem::pending_bits)
    }

    /// Whether there is anything to send bits into.
    ///
    /// False while a retrain is running phase 2 again: there is no data mode
    /// to carry them, [`Self::send_bits`] drops what it is given and
    /// [`Self::pending_bits`] stays at zero. Whatever is above has to know
    /// that, or it will feed a transmitter that never fills.
    pub fn accepts_bits(&self) -> bool {
        self.training.is_some()
    }

    /// Start a rate renegotiation from data mode, offering to receive no
    /// faster than `receive` times 2400 bit/s. False outside data mode.
    pub fn renegotiate(&mut self, receive: u8) -> bool {
        self.training.as_mut().is_some_and(|t| t.renegotiate(receive))
    }

    /// Rate renegotiations and cleardowns since the call began.
    pub fn renegotiations(&self) -> u32 {
        self.training.as_ref().map_or(0, training::Modem::renegotiations)
    }

    /// Clear the call down from data mode (11.7). False outside data mode.
    pub fn clear_down(&mut self) -> bool {
        self.training.as_mut().is_some_and(training::Modem::clear_down)
    }

    /// Whether the far end's data signal is there.
    pub fn carrier(&self) -> bool {
        self.training.as_ref().is_some_and(training::Modem::carrier)
    }

    /// The far end's last symbol: phase 2's DPSK, each symbol against the
    /// one before, and from phase 3 on what the trained receiver decided on.
    pub fn constellation_point(&self) -> Option<(f64, f64)> {
        match self.training.as_ref() {
            Some(training) => training.constellation_point(),
            None => self.phase2.scope_point(),
        }
    }

    /// Points in the constellation the far end is read against, once phase 3
    /// has trained the receiver.
    pub fn constellation_size(&self) -> Option<usize> {
        self.training.as_ref().map(training::Modem::constellation_size)
    }

    /// Signal to noise of the decisions being made, in decibels, once phase 3
    /// has trained the receiver.
    pub fn snr_db(&self) -> Option<f64> {
        self.training.as_ref().map(training::Modem::snr)
    }

    /// The largest coordinate those points reach, in the units of
    /// [`Self::constellation_point`].
    pub fn constellation_peak(&self) -> Option<f64> {
        self.training.as_ref().map(training::Modem::constellation_peak)
    }

    pub fn phase(&self) -> &'static str {
        match self.training.as_ref() {
            Some(training) => training.phase(),
            None => self.phase2.phase(),
        }
    }

    /// Ask for V.34 in phase 2's INFO1a from now on, even of a V.90 server.
    pub fn decline_pcm(&mut self) {
        self.phase2.decline_pcm();
    }

    /// Ask for V.90 data mode rather than PCM upstream in phase 2's INFO1a
    /// from now on, even of a V.92 server that has offered it.
    ///
    /// The rung above [`Self::decline_pcm`]: PCM downstream is kept and only
    /// the upstream falls back to V.34 modulation.
    pub fn decline_pcm_upstream(&mut self) {
        self.phase2.decline_pcm_upstream();
    }

    /// Start phase 2 again as a retrain, from wherever this start-up is.
    pub fn restart_phase2(&mut self) {
        self.phase2 = self.phase2.again();
        self.training = None;
        self.retraining = true;
        self.retrains += 1;
    }

    /// Carry the start-up one sample further.
    pub fn step(&mut self, line: f64) -> f64 {
        let cleaned = self.cancel(line);
        let out = self.step_cleaned(cleaned);
        self.sent = out;
        out
    }

    /// The start-up itself, on a line with this end's echo taken out.
    fn step_cleaned(&mut self, line: f64) -> f64 {
        if let Some(training) = self.training.as_mut() {
            let out = training.step(line);
            if training.take_retrain() {
                // 11.5: go back to phase 2, keeping the capabilities the first
                // start-up settled -- a retrain does not exchange INFO0 again.
                // The very sample is the retrain's first, so its tone follows
                // the data with no gap the far end has to wait through.
                let failed_at = (self.reached_phase4 && !self.reached_data).then(|| training.settings());
                self.phase2 = self.phase2.again();
                // A start-up that trained at its symbol rates both ways and
                // still never reached data mode is not run at them again
                // while others will do: the far end would very likely say the
                // same in INFO1c and fail the same way (see
                // `phase2::Modem::avoid_symbol_rate`). One that came back
                // before phase 4 -- a far end that lost INFO1a, say -- says
                // nothing about them.
                if let Some(settings) = failed_at {
                    self.phase2.avoid_symbol_rate(settings.transmit.rate);
                    self.phase2.avoid_symbol_rate(settings.receive.rate);
                }
                self.training = None;
                self.retraining = true;
                self.retrains += 1;
                return self.phase2.step(line);
            }
            self.reached_phase4 |= training.in_phase4();
            // The recovery is over the moment the new training connects.
            if matches!(training.status(), training::Status::Connected { .. }) {
                self.retraining = false;
                self.phase2_retrains = 0;
                self.reached_data = true;
            }
            return out;
        }
        let out = self.phase2.step(line);
        // 11.2.2: a phase 2 that lost its place is retrained, not given up
        // on. live-1789546478 lost a whole call to one INFO1a a VoIP slip
        // had damaged, with the far end still there and waiting.
        if self.phase2.asks_for_retrain()
            && self.phase2_retrains < PHASE2_RETRAINS
            && self.phase2.far_capabilities().is_some()
        {
            self.phase2 = self.phase2.again();
            self.phase2_retrains += 1;
            if self.retraining {
                self.retrains += 1;
            }
            return out;
        }
        if self.phase2.status() == phase2::Status::Done {
            self.training = self.settings().map(|settings| training::Modem::new(settings, self.fs));
            self.reached_phase4 = false;
            self.reached_data = false;
        }
        out
    }

    /// What phases 3 and 4 run on, from what phase 2 left.
    fn settings(&self) -> Option<Settings> {
        let far = self.phase2.far_capabilities()?;
        let info1c = self.phase2.info1c()?;
        let info1a = self.phase2.info1a()?;
        // This end has the 1664-point constellation, as its INFO0 says.
        let wide = far.constellation_1664;
        Some(Settings::new(self.phase2.role(), &far, &info1c, &info1a, self.phase2.round_trip().unwrap_or(0.0), wide))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v34::signals::Size;

    const FS: f64 = 16_000.0;

    /// Both ends against each other through a delay and a little noise, with
    /// what the answer modem put on the line kept.
    fn run(one_way: f64, seconds: f64) -> (Modem, Modem, Vec<f64>, Vec<&'static str>) {
        let delay = (one_way * FS) as usize;
        let mut caller = Modem::new(Role::Call, FS);
        let mut answerer = Modem::new(Role::Answer, FS);
        let mut to_answer: std::collections::VecDeque<f64> = std::iter::repeat_n(0.0, delay).collect();
        let mut to_call: std::collections::VecDeque<f64> = std::iter::repeat_n(0.0, delay).collect();
        let mut seed = 7u32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 2e-4
        };
        let mut phases = Vec::new();
        let mut answered = Vec::new();
        for _ in 0..(seconds * FS) as usize {
            let from_call = caller.step(to_call.pop_front().unwrap() * 0.3 + noise());
            let from_answer = answerer.step(to_answer.pop_front().unwrap() * 0.3 + noise());
            to_answer.push_back(from_call);
            to_call.push_back(from_answer);
            answered.push(from_answer);
            if phases.last() != Some(&caller.phase()) {
                phases.push(caller.phase());
            }
            if caller.status() != Status::Running && answerer.status() != Status::Running {
                break;
            }
        }
        (caller, answerer, answered, phases)
    }

    #[test]
    fn two_ends_go_from_info0_to_e() {
        let (caller, answerer, _, phases) = run(0.030, 25.0);
        assert!(matches!(caller.status(), Status::Connected { .. }), "call modem went {phases:?}");
        assert!(matches!(answerer.status(), Status::Connected { .. }), "answer modem stuck at {}", answerer.phase());
        let (call, answer) = (caller.training().unwrap(), answerer.training().unwrap());
        assert_eq!(call.far_asked(), Some(Size::Sixteen));
        assert_eq!(call.rates(), answer.rates().map(|(tx, rx)| (rx, tx)));
        // Phase 2 settled 3429 symbols a second both ways on a clean line,
        // and that is what phases 3 and 4 ran at.
        assert_eq!(call.settings().transmit.rate, crate::v34::info::SymbolRate::S3429);
        assert_eq!(call.rates(), Some((14, 14)));
    }

    #[test]
    fn data_crosses_both_ways_at_33600() {
        let delay = (0.030 * FS) as usize;
        let mut caller = Modem::new(Role::Call, FS);
        let mut answerer = Modem::new(Role::Answer, FS);
        let mut to_answer: std::collections::VecDeque<f64> = std::iter::repeat_n(0.0, delay).collect();
        let mut to_call: std::collections::VecDeque<f64> = std::iter::repeat_n(0.0, delay).collect();
        let from_call: Vec<bool> = (0..3000).map(|i| (i * 37 + 11) % 7 < 3).collect();
        let from_answer: Vec<bool> = (0..3000).map(|i| (i * 13 + 5) % 5 < 2).collect();
        let (mut at_call, mut at_answer) = (Vec::new(), Vec::new());
        let mut sent = false;
        let mut after = 0;
        for _ in 0..(25.0 * FS) as usize {
            let out_call = caller.step(to_call.pop_front().unwrap() * 0.3);
            let out_answer = answerer.step(to_answer.pop_front().unwrap() * 0.3);
            to_answer.push_back(out_call);
            to_call.push_back(out_answer);
            let up = |m: &Modem| matches!(m.status(), Status::Connected { .. });
            if up(&caller) && up(&answerer) {
                if !sent {
                    caller.take_bits();
                    answerer.take_bits();
                    caller.send_bits(&from_call);
                    answerer.send_bits(&from_answer);
                    sent = true;
                }
                after += 1;
                at_call.extend(caller.take_bits());
                at_answer.extend(answerer.take_bits());
                // The scope's constellation: 1408 points, minimum shaping.
                assert_eq!(caller.constellation_size(), Some(1408));
                assert!(caller.constellation_peak().is_some_and(|p| (1.3..2.0).contains(&p)), "{:?}", caller.constellation_peak());
                if after > (0.5 * FS) as usize {
                    break;
                }
            }
        }
        assert!(sent, "never connected: {} and {}", caller.phase(), answerer.phase());
        let contains = |haystack: &[bool], needle: &[bool]| haystack.windows(needle.len()).any(|w| w == needle);
        assert!(contains(&at_answer, &from_call), "call to answer lost ({} bits)", at_answer.len());
        assert!(contains(&at_call, &from_answer), "answer to call lost ({} bits)", at_call.len());
    }

    /// A call that is up, a retrain, and the call up again -- the whole of
    /// 11.5, both ends going back through phase 2 on the capabilities they
    /// already have and training again, without a byte of the capabilities
    /// exchange repeated.
    #[test]
    fn a_retrain_takes_a_connected_call_back_through_phase_2_and_up_again() {
        let delay = (0.030 * FS) as usize;
        let mut caller = Modem::new(Role::Call, FS);
        let mut answerer = Modem::new(Role::Answer, FS);
        let mut to_answer: std::collections::VecDeque<f64> = std::iter::repeat_n(0.0, delay).collect();
        let mut to_call: std::collections::VecDeque<f64> = std::iter::repeat_n(0.0, delay).collect();
        let up = |m: &Modem| matches!(m.status(), Status::Connected { .. });

        let mut asked = false;
        let mut dropped = false;
        let mut reconnected_at = None;
        for i in 0..(45.0 * FS) as usize {
            let out_call = caller.step(to_call.pop_front().unwrap() * 0.3);
            let out_answer = answerer.step(to_answer.pop_front().unwrap() * 0.3);
            to_answer.push_back(out_call);
            to_call.push_back(out_answer);
            // Once both are in data mode, the answer modem starts a retrain.
            if !asked && up(&caller) && up(&answerer) {
                asked = true;
                assert!(answerer.retrain(), "a connected modem would not retrain");
            }
            // Both must leave data mode -- the call is recovering, not up.
            if asked && !dropped {
                if matches!(caller.status(), Status::Retraining)
                    && matches!(answerer.status(), Status::Retraining)
                {
                    dropped = true;
                }
            } else if dropped && up(&caller) && up(&answerer) {
                reconnected_at = Some(i as f64 / FS);
                break;
            }
        }
        assert!(asked, "never connected in the first place");
        assert!(dropped, "the retrain never took the call back to phase 2");
        let at = reconnected_at.expect("the call never came back up after the retrain");
        assert!(matches!(caller.status(), Status::Connected { .. }));
        assert!(matches!(answerer.status(), Status::Connected { .. }));
        // The retrain settled the same rates the first start-up did.
        let (call, answer) = (caller.training().unwrap(), answerer.training().unwrap());
        assert_eq!(call.rates(), Some((14, 14)));
        assert_eq!(answer.rates(), Some((14, 14)));
        println!("  back up at {at:.1}s");
    }

    /// INFO1a is sent once and nothing repeats it. A VoIP jitter buffer's
    /// slip through the middle of it cost a live call (live-1789546478) that
    /// was in a retrain: the call modem gave up on it and hung up, with the
    /// far end still there. 11.2.2.1.6 retrains instead, and the answer modem
    /// hears the call modem's tone B from phase 3 and comes back too.
    #[test]
    fn an_info1a_lost_on_the_way_is_retrained_from() {
        let delay = (0.030 * FS) as usize;
        let mut caller = Modem::new(Role::Call, FS);
        let mut answerer = Modem::new(Role::Answer, FS);
        let mut to_answer: std::collections::VecDeque<f64> = std::iter::repeat_n(0.0, delay).collect();
        let mut to_call: std::collections::VecDeque<f64> = std::iter::repeat_n(0.0, delay).collect();
        let up = |m: &Modem| matches!(m.status(), Status::Connected { .. });
        // When the answer modem began INFO1a, by its own clock.
        let mut info1a_at = None;
        let mut failed = None;
        for i in 0..(60.0 * FS) as usize {
            let out_call = caller.step(to_call.pop_front().unwrap() * 0.3);
            let mut out_answer = answerer.step(to_answer.pop_front().unwrap() * 0.3);
            if info1a_at.is_none() && answerer.phase2().info1a().is_some() {
                info1a_at = Some(i);
            }
            // Forty milliseconds in, the next thirty never arrive: the sync
            // and the CRC can no longer agree.
            if let Some(at) = info1a_at
                && (at + (0.040 * FS) as usize..at + (0.070 * FS) as usize).contains(&i)
            {
                out_answer = 0.0;
            }
            to_answer.push_back(out_call);
            to_call.push_back(out_answer);
            if let Status::Failed(why) = caller.status() {
                failed = Some(why);
                break;
            }
            if up(&caller) && up(&answerer) {
                break;
            }
        }
        assert!(info1a_at.is_some(), "the answer modem never sent INFO1a");
        assert_eq!(failed, None, "the call modem gave up");
        assert!(up(&caller) && up(&answerer), "never came up: {} and {}", caller.phase(), answerer.phase());
        // Phase 2 was run again by both; phase 3 and 4 settled as normal.
        assert!(caller.phase2().info1a().is_some());
        assert_eq!(caller.training().unwrap().rates(), Some((14, 14)));
    }

    #[test]
    fn the_answer_modem_leaves_70_ms_between_info1a_and_s() {
        // "After sending sequence INFO1a, the modem shall transmit silence for
        // 70 ± 5 ms, signal S for 128T" (11.3.1.2.1). INFO1a is the last thing
        // the answer modem sends in phase 2, so the silence is the first gap
        // of more than a few milliseconds after its INFO1c arrives -- which is
        // late in the call, past the probing and its silences.
        let (_, answerer, answered, _) = run(0.030, 25.0);
        assert!(matches!(answerer.status(), Status::Connected { .. }));
        let loud: Vec<bool> = answered.iter().map(|x| x.abs() > 1e-3).collect();
        // Every gap of more than 20 ms, as (start, length) in samples.
        let mut gaps = Vec::new();
        let mut start = None;
        for (i, &l) in loud.iter().enumerate() {
            match (l, start) {
                (false, None) => start = Some(i),
                (true, Some(s)) => {
                    if i - s > (0.020 * FS) as usize {
                        gaps.push((s, i - s));
                    }
                    start = None;
                }
                _ => {}
            }
        }
        // The last gap before phase 3's S: phase 4's S follows a silence of
        // round trips, and INFO1a's is the one before that.
        let silences: Vec<f64> = gaps.iter().map(|(_, n)| *n as f64 / FS * 1000.0).collect();
        let before_s = gaps.iter().rev().nth(1).map(|(_, n)| *n as f64 / FS * 1000.0).expect("no gaps");
        assert!((65.0..=75.0).contains(&before_s), "{before_s:.1} ms; gaps {silences:?}");
    }
}
