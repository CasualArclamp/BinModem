//! V.90 from the end of V.8 to data: V.90's phase 2, then V.90's phases 3
//! and 4 -- or V.34's, when the far end turns out not to be a V.90 digital
//! modem (9.2.1.1.8, 9.2.2.1.9).
//!
//! Both ends run V.34's start-up with V.90's phase 2 inside it. A far end
//! that is not a V.90 pair for this one leaves phase 2 with V.34's INFO1a,
//! and V.34's start-up carries on as it would have; a far end that is leaves
//! it with V.90's, and V.90 takes over from there. A V.90 start-up that
//! loses its place retrains, back through V.90's phase 2 (9.5): "Any
//! subsequent retrains shall use Phase 2 of V.90". A line that will not
//! carry PCM at all -- the DIL says so, or V.90 has failed too often -- gets
//! V.34's INFO1a in that phase 2 instead, and the call goes on as V.34.

use crate::v34::info::Info0d;
use crate::v34::phase2::{self, Pcm};
use crate::v34::startup as v34;

use super::ucode::Law;
use super::{analogue, digital};

/// Retrains in a row a failed V.90 start-up gets before the analogue modem
/// asks for V.34 instead.
const V90_RETRAINS: u32 = 2;

/// How the start-up is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    /// In data mode, at these rates in bit/s.
    Connected { transmit: u32, receive: u32 },
    /// Going through the start-up again, or renegotiating the rates, with a
    /// call that was up.
    Retraining,
    /// The call ended the way 9.7 (or V.34's 11.7) ends one.
    ClearedDown,
    Failed(&'static str),
}

/// The analogue modem's start-up: the end that dials an ISP.
#[derive(Debug, Clone)]
pub struct Analogue {
    fs: f64,
    v34: v34::Modem,
    v90: Option<analogue::Modem>,
    /// V.90 start-ups that have failed in a row.
    failed_starts: u32,
    connected_once: bool,
    last_failure: Option<&'static str>,
    /// Renegotiations in V.90 data modes a retrain has since replaced.
    renegotiations: u32,
    holes: u32,
}

impl Analogue {
    /// From the 75 ms of silence that end phase 1.
    pub fn new(fs: f64) -> Self {
        Self {
            fs,
            v34: v34::Modem::with_phase2(phase2::Modem::v90(Pcm::Analogue, fs), fs),
            v90: None,
            failed_starts: 0,
            connected_once: false,
            last_failure: None,
            renegotiations: 0,
            holes: 0,
        }
    }

    /// Whether V.90 is what the call came to.
    pub fn is_v90(&self) -> bool {
        self.v90.is_some()
    }

    /// V.34's start-up, which holds phase 2 and a V.34 call if that is what
    /// this became.
    pub fn v34(&self) -> &v34::Modem {
        &self.v34
    }

    pub fn v34_mut(&mut self) -> &mut v34::Modem {
        &mut self.v34
    }

    /// V.90's phases 3 and 4 and data mode, once phase 2 has settled on V.90.
    pub fn v90(&self) -> Option<&analogue::Modem> {
        self.v90.as_ref()
    }

    /// Full retrains since the call began.
    pub fn retrains(&self) -> u32 {
        // Every one of them, V.90's included, goes back through V.34's
        // start-up's phase 2.
        self.v34.retrains()
    }

    /// Holes in the audio seen in data mode since the call began (see
    /// [`analogue::Modem::holes`]), this start-up's and every one before it.
    pub fn holes(&self) -> u32 {
        self.holes + self.v90.as_ref().map_or(0, analogue::Modem::holes)
    }

    /// Why the last V.90 start-up failed, if one has.
    pub fn last_failure(&self) -> Option<&'static str> {
        self.last_failure
    }

    /// Rate renegotiations and cleardowns since the call began, V.90's and
    /// V.34's.
    pub fn renegotiations(&self) -> u32 {
        self.renegotiations + self.v90.as_ref().map_or(0, analogue::Modem::renegotiations) + self.v34.renegotiations()
    }

    /// Renegotiate from data mode, asking to receive no faster than
    /// `receive` bit/s (V.90 9.6.2.1, or V.34 11.6.1.1). False outside data
    /// mode.
    pub fn renegotiate(&mut self, receive: u32) -> bool {
        match self.v90.as_mut() {
            Some(m) => m.renegotiate(receive),
            None => self.v34.renegotiate((receive / 2400).clamp(1, 14) as u8),
        }
    }

    /// End the call from data mode (V.90 9.7, or V.34 11.7).
    pub fn clear_down(&mut self) -> bool {
        match self.v90.as_mut() {
            Some(m) => m.clear_down(),
            None => self.v34.clear_down(),
        }
    }

    /// Back through V.90's phase 2.
    fn back_to_phase2(&mut self) {
        if let Some(m) = self.v90.take() {
            self.renegotiations += m.renegotiations();
            self.holes += m.holes();
        }
        self.v34.restart_phase2();
    }

    pub fn status(&self) -> Status {
        match self.v90.as_ref().map(analogue::Modem::status) {
            Some(analogue::Status::Connected { downstream, upstream }) => {
                Status::Connected { transmit: upstream, receive: downstream }
            }
            Some(analogue::Status::Failed(why)) => Status::Failed(why),
            Some(analogue::Status::ClearedDown) => Status::ClearedDown,
            Some(analogue::Status::Running) if self.connected_once => Status::Retraining,
            Some(analogue::Status::Running) => Status::Running,
            None => match self.v34.status() {
                v34::Status::Running if self.connected_once => Status::Retraining,
                v34::Status::Running | v34::Status::Done => Status::Running,
                v34::Status::Connected { transmit, receive } => Status::Connected { transmit, receive },
                v34::Status::Retraining => Status::Retraining,
                v34::Status::ClearedDown => Status::ClearedDown,
                v34::Status::Failed(why) => Status::Failed(why),
            },
        }
    }

    pub fn phase(&self) -> &'static str {
        match self.v90.as_ref() {
            Some(m) => m.phase(),
            None if self.v34.training().is_none() => match self.v34.phase2().status() {
                phase2::Status::Failed(_) => "V.90 phase 2 failed",
                _ => "V.90 phase 2",
            },
            None => self.v34.phase(),
        }
    }

    pub fn round_trip(&self) -> Option<f64> {
        self.v34.phase2().round_trip()
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        match self.v90.as_mut() {
            Some(m) => m.take_bits(),
            None => self.v34.take_bits(),
        }
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        match self.v90.as_mut() {
            Some(m) => m.send_bits(bits),
            None => self.v34.send_bits(bits),
        }
    }

    pub fn pending_bits(&self) -> usize {
        match self.v90.as_ref() {
            Some(m) => m.pending_bits(),
            None => self.v34.pending_bits(),
        }
    }

    /// Whether there is anything to send bits into.
    pub fn accepts_bits(&self) -> bool {
        match self.v90.as_ref() {
            Some(m) => matches!(m.status(), analogue::Status::Connected { .. }),
            None => self.v34.accepts_bits(),
        }
    }

    /// Whether the far end's data signal is there.
    pub fn carrier(&self) -> bool {
        match self.v90.as_ref() {
            Some(m) => m.carrier(),
            None => self.v34.carrier(),
        }
    }

    /// Start a full retrain (9.5.2.1): back through V.90's phase 2. False if
    /// there is no call to retrain.
    pub fn retrain(&mut self) -> bool {
        match self.v90.as_mut() {
            Some(m) => {
                m.start_retrain();
                true
            }
            None => self.v34.retrain(),
        }
    }

    /// Carry the start-up one sample further.
    pub fn step(&mut self, line: f64) -> f64 {
        if let Some(m) = self.v90.as_mut() {
            let out = m.step(line);
            if m.take_retrain() {
                // 9.5.2: tone A and phase 2, whichever end began it; the
                // capabilities are not exchanged again.
                self.back_to_phase2();
                return out;
            }
            match m.status() {
                analogue::Status::Connected { .. } => {
                    self.connected_once = true;
                    self.failed_starts = 0;
                }
                analogue::Status::Failed(why) => {
                    // 9.5.2.1: back to V.90's phase 2.
                    let hopeless = m.route().is_some() && m.choice().is_none();
                    self.last_failure = Some(why);
                    self.failed_starts += 1;
                    self.back_to_phase2();
                    if hopeless || self.failed_starts > V90_RETRAINS {
                        // 9.2.2.1.9: this time, V.34's INFO1a.
                        self.v34.decline_pcm();
                    }
                }
                _ => {}
            }
            return out;
        }
        let out = self.v34.step(line);
        if matches!(self.v34.status(), v34::Status::Connected { .. }) {
            self.connected_once = true;
        }
        let p2 = self.v34.phase2();
        if p2.status() == phase2::Status::Done
            && self.v34.training().is_none()
            && let (Some(asked), Some(server), Some(info1d)) = (p2.info1a_pcm(), p2.far_info0d(), p2.info1c())
        {
            let ours_wide = true;
            let mut settings = analogue::Settings::new(&server, &info1d, &asked, p2.round_trip().unwrap_or(0.0), ours_wide);
            settings.v34_receive = p2.v34_receive_rate().unwrap_or(0);
            self.v90 = Some(analogue::Modem::new(settings, self.fs));
        }
        out
    }
}

/// The digital modem's start-up, at the network's rate: a V.90 server, here
/// for the analogue modem to be tested against.
#[derive(Debug, Clone)]
pub struct Digital {
    law: Law,
    v34: v34::Modem,
    v90: Option<digital::Modem>,
    /// Phase 2 goes out through the codec like anything else, at the power
    /// INFO0d names; so does V.34, if that is what the call became.
    phase2_gain: f64,
    /// V.90 start-ups that have failed in a row.
    failed_starts: u32,
    /// Renegotiations in V.90 data modes a retrain has since replaced.
    renegotiations: u32,
    habits: digital::Habits,
    /// Whether data mode has been reached, after which a start-up is a
    /// retrain or a renegotiation rather than the call being placed.
    connected_once: bool,
}

impl Digital {
    pub fn new(info0d: Info0d) -> Self {
        let law = if info0d.a_law { Law::A } else { Law::Mu };
        // A full-scale sine is +3.17 dBm0 in G.711.
        let phase2_gain = 10f64.powf((info0d.nominal_dbm0() - 3.17) / 20.0);
        Self {
            law,
            v34: v34::Modem::with_phase2(phase2::Modem::v90(Pcm::Digital(info0d), digital::FS), digital::FS),
            v90: None,
            phase2_gain,
            failed_starts: 0,
            renegotiations: 0,
            habits: digital::Habits::default(),
            connected_once: false,
        }
    }

    /// Go about phase 3 this way.
    pub fn with_habits(mut self, habits: digital::Habits) -> Self {
        self.habits = habits;
        self
    }

    pub fn v34(&self) -> &v34::Modem {
        &self.v34
    }

    pub fn v90(&self) -> Option<&digital::Modem> {
        self.v90.as_ref()
    }

    /// Whether V.90 is what the call came to.
    pub fn is_v90(&self) -> bool {
        self.v90.is_some()
    }

    pub fn round_trip(&self) -> Option<f64> {
        self.v34.phase2().round_trip()
    }

    /// Whether there is anything to send bits into.
    pub fn accepts_bits(&self) -> bool {
        match self.v90.as_ref() {
            Some(m) => matches!(m.status(), digital::Status::Connected { .. }),
            None => self.v34.accepts_bits(),
        }
    }

    /// Whether the far end's data signal is there.
    pub fn carrier(&self) -> bool {
        match self.v90.as_ref() {
            Some(m) => m.carrier(),
            None => self.v34.carrier(),
        }
    }

    /// Full retrains since the call began.
    pub fn retrains(&self) -> u32 {
        self.v34.retrains()
    }

    /// Rate renegotiations and cleardowns since the call began, V.90's and
    /// V.34's.
    pub fn renegotiations(&self) -> u32 {
        self.renegotiations + self.v90.as_ref().map_or(0, digital::Modem::renegotiations) + self.v34.renegotiations()
    }

    /// The analogue modem's last upstream symbol, for a scope.
    pub fn constellation_point(&self) -> Option<(f64, f64)> {
        match self.v90.as_ref() {
            Some(m) => m.last_point(),
            None => self.v34.constellation_point(),
        }
    }

    /// The largest coordinate the upstream's points reach, in the units of
    /// [`Self::constellation_point`].
    pub fn constellation_peak(&self) -> Option<f64> {
        match self.v90.as_ref() {
            Some(m) => Some(m.upstream_peak()),
            None => self.v34.constellation_peak(),
        }
    }

    /// Points the upstream is read against.
    pub fn constellation_size(&self) -> Option<usize> {
        match self.v90.as_ref() {
            Some(m) => Some(m.upstream_points()),
            None => self.v34.constellation_size(),
        }
    }

    pub fn status(&self) -> Status {
        match self.v90.as_ref().map(digital::Modem::status) {
            Some(digital::Status::Connected { downstream, upstream }) => {
                Status::Connected { transmit: downstream, receive: upstream }
            }
            Some(digital::Status::Failed(why)) => Status::Failed(why),
            Some(digital::Status::ClearedDown) => Status::ClearedDown,
            // A renegotiation, or phases 3 and 4 again after a retrain: the
            // call is still up, as the analogue modem's start-up says too.
            // Reported as the start-up it is, a renegotiation looked to the
            // modem above like a call with no carrier, and ended it.
            Some(digital::Status::Running) if self.connected_once => Status::Retraining,
            Some(digital::Status::Running) => Status::Running,
            None => match self.v34.status() {
                v34::Status::Connected { transmit, receive } => Status::Connected { transmit, receive },
                v34::Status::Failed(why) => Status::Failed(why),
                v34::Status::Retraining => Status::Retraining,
                v34::Status::ClearedDown => Status::ClearedDown,
                _ if self.connected_once => Status::Retraining,
                _ => Status::Running,
            },
        }
    }

    pub fn phase(&self) -> &'static str {
        match self.v90.as_ref() {
            Some(m) => m.phase(),
            None if self.v34.training().is_none() => "V.90 phase 2",
            None => self.v34.phase(),
        }
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        match self.v90.as_mut() {
            Some(m) => m.take_bits(),
            None => self.v34.take_bits(),
        }
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        match self.v90.as_mut() {
            Some(m) => m.send_bits(bits),
            None => self.v34.send_bits(bits),
        }
    }

    pub fn pending_bits(&self) -> usize {
        match self.v90.as_ref() {
            Some(m) => m.pending_bits(),
            None => self.v34.pending_bits(),
        }
    }

    /// Start a full retrain (9.5.1.1).
    pub fn retrain(&mut self) -> bool {
        match self.v90.as_mut() {
            Some(m) => {
                m.start_retrain();
                true
            }
            None => self.v34.retrain(),
        }
    }

    /// Renegotiate from data mode (9.6.1.1), asking the analogue modem to
    /// send no faster than `upstream`, a multiple of 2400.
    pub fn renegotiate(&mut self, upstream: u8) -> bool {
        match self.v90.as_mut() {
            Some(m) => m.renegotiate(upstream),
            None => self.v34.renegotiate(upstream),
        }
    }

    /// End the call from data mode (9.7).
    pub fn clear_down(&mut self) -> bool {
        match self.v90.as_mut() {
            Some(m) => m.clear_down(),
            None => self.v34.clear_down(),
        }
    }

    /// One network sample in, one out.
    pub fn step(&mut self, input: f64) -> f64 {
        if let Some(m) = self.v90.as_mut() {
            let out = m.step(input);
            let failed = matches!(m.status(), digital::Status::Failed(_));
            if matches!(m.status(), digital::Status::Connected { .. }) {
                self.failed_starts = 0;
                self.connected_once = true;
            }
            if m.take_retrain() || (failed && self.failed_starts < V90_RETRAINS) {
                // 9.5.1: tone B and phase 2.
                self.failed_starts += u32::from(failed);
                self.renegotiations += m.renegotiations();
                self.v90 = None;
                self.v34.restart_phase2();
            }
            return out;
        }
        let out = self.v34.step(input);
        if matches!(self.v34.status(), v34::Status::Connected { .. }) {
            self.connected_once = true;
        }
        let p2 = self.v34.phase2();
        if p2.status() == phase2::Status::Done
            && self.v34.training().is_none()
            && let (Some(asked), Some(info1d)) = (p2.info1a_pcm(), p2.info1c())
        {
            let wide = p2.far_capabilities().is_some_and(|f| f.constellation_1664);
            let mut settings = digital::Settings::new(self.law, &info1d, &asked, p2.round_trip().unwrap_or(0.0), wide);
            settings.habits = self.habits;
            self.v90 = Some(digital::Modem::new(settings));
        }
        // Everything that is not V.90's codewords goes out at that power:
        // phase 2, and V.34 if that is what the call became.
        out * self.phase2_gain
    }
}
