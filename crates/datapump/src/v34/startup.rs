//! V.34 from the end of V.8 to the start of data: phase 2, and phases 3 and
//! 4 straight after it on what phase 2 settled.

use super::phase2::{self, Role};
use super::training::{self, Settings};

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
        }
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

    /// The far end's last symbol, once phase 3 has trained the receiver.
    pub fn constellation_point(&self) -> Option<(f64, f64)> {
        self.training.as_ref().and_then(training::Modem::constellation_point)
    }

    /// Points in the constellation the far end is read against, once phase 3
    /// has trained the receiver.
    pub fn constellation_size(&self) -> Option<usize> {
        self.training.as_ref().map(training::Modem::constellation_size)
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

    /// Start phase 2 again as a retrain, from wherever this start-up is.
    pub fn restart_phase2(&mut self) {
        self.phase2 = self.phase2.again();
        self.training = None;
        self.retraining = true;
        self.retrains += 1;
    }

    /// Carry the start-up one sample further.
    pub fn step(&mut self, line: f64) -> f64 {
        if let Some(training) = self.training.as_mut() {
            let out = training.step(line);
            if training.take_retrain() {
                // 11.5: go back to phase 2, keeping the capabilities the first
                // start-up settled -- a retrain does not exchange INFO0 again.
                // The very sample is the retrain's first, so its tone follows
                // the data with no gap the far end has to wait through.
                self.phase2 = self.phase2.again();
                self.training = None;
                self.retraining = true;
                self.retrains += 1;
                return self.phase2.step(line);
            }
            // The recovery is over the moment the new training connects.
            if matches!(training.status(), training::Status::Connected { .. }) {
                self.retraining = false;
                self.phase2_retrains = 0;
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
