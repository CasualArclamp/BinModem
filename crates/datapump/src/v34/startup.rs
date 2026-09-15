//! V.34 from the end of V.8 to the start of data: phase 2, and phases 3 and
//! 4 straight after it on what phase 2 settled.

use super::phase2::{self, Role};
use super::training::{self, Settings};

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
}

impl Modem {
    pub fn new(role: Role, fs: f64) -> Self {
        Self { fs, phase2: phase2::Modem::new(role, fs), training: None }
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
            _ => Status::Running,
        }
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

    /// Carry the start-up one sample further.
    pub fn step(&mut self, line: f64) -> f64 {
        if let Some(training) = self.training.as_mut() {
            return training.step(line);
        }
        let out = self.phase2.step(line);
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
