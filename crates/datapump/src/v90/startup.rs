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
use crate::v34::phase2::{self, Pcm, V92Wish};
use crate::v34::startup as v34;

use crate::v92::digital as pcm;
use crate::v92::{FilterLimits, analogue as pcm_up, line as pcm_line};

use super::sequences::Descriptor;
use super::ucode::Law;
use super::{analogue, digital};

/// Retrains in a row a failed V.90 start-up gets before the analogue modem
/// asks for V.34 instead.
const V90_RETRAINS: u32 = 2;

/// Failed PCM-upstream start-ups in a row before the analogue modem asks for
/// V.90's Table 10 instead, keeping PCM downstream: the rung above
/// [`V90_RETRAINS`] on the ladder (see `phase2::Modem::decline_pcm_upstream`).
const V92_RETRAINS: u32 = 2;

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
    /// Lines for the transcript not yet taken (see [`Self::take_notes`]).
    notes: Vec<String>,
    /// The rate every V.90 start-up on this call asks for, as its drn, or
    /// None for the DIL's own choice (see [`analogue::Settings::pinned`]).
    pinned: Option<u8>,
    /// V.92's Phases 3 and 4 at the line's rate, once a phase 2 has ended in
    /// a Table 18 INFO1a (9.5.2, 9.6.2). At most one of this and `v90` is
    /// ever built.
    v92: Option<Box<pcm_line::Line>>,
    /// PCM-upstream start-ups that have failed in a row.
    failed_v92: u32,
}

impl Analogue {
    /// From the 75 ms of silence that end phase 1.
    pub fn new(fs: f64, lead_in: f64) -> Self {
        Self {
            fs,
            v34: v34::Modem::with_phase2(phase2::Modem::v90(Pcm::Analogue, fs, lead_in), fs),
            v90: None,
            failed_starts: 0,
            connected_once: false,
            last_failure: None,
            renegotiations: 0,
            holes: 0,
            notes: Vec::new(),
            pinned: None,
            v92: None,
            failed_v92: 0,
        }
    }

    /// Offer V.92 from this end: INFO0a's capability bit (Table 16/V.92), and
    /// PCM upstream with a Table 18 INFO1a if the digital modem's INFO1d
    /// allows it (9.3, 8.4.1).
    ///
    /// Phase 2 is rebuilt with V.92's INFO0a in it, so call this before the
    /// call is placed. Everything after phase 2 follows the INFO1a that went:
    /// Table 18 is V.92's own Phases 3 and 4 ([`pcm_line::Line`]), Table 10 is
    /// V.90 exactly as before, and V.34's is V.34.
    pub fn with_v92(mut self, wish: V92Wish, lead_in: f64) -> Self {
        self.v34 = v34::Modem::with_phase2(phase2::Modem::v90(Pcm::AnalogueV92(wish), self.fs, lead_in), self.fs);
        self
    }

    /// Whether V.92's PCM upstream is what the call came to.
    pub fn is_v92(&self) -> bool {
        self.v92.is_some()
    }

    /// V.92's Phases 3 and 4, once PCM upstream is what the call came to.
    pub fn v92(&self) -> Option<&pcm_up::Modem> {
        self.v92.as_deref().map(pcm_line::Line::modem)
    }

    /// How many bits one upstream data frame takes, for a caller keeping the
    /// queue fed (see [`pcm_up::Modem::fill_bits`]). Zero outside V.92's data
    /// mode.
    pub fn fill_bits(&self) -> usize {
        self.v92().map_or(0, pcm_up::Modem::fill_bits)
    }

    /// The rate V.90 start-ups are to ask for, as its drn, or None for the
    /// DIL's own choice: from the window, before the call. This start-up's
    /// too, if its DIL has not yet been read.
    pub fn set_pinned(&mut self, drn: Option<u8>) {
        self.pinned = drn;
        if let Some(m) = self.v90.as_mut() {
            m.set_pinned(drn);
        }
    }

    /// Whether V.90's data mode has been reached on this start-up, so that
    /// the rate menu renegotiates rather than pins.
    pub fn data_mode_reached(&self) -> bool {
        self.v90.as_ref().is_some_and(analogue::Modem::data_mode_reached)
    }

    /// The rate menu in data mode: a rate renegotiation to `drn` (see
    /// [`analogue::Modem::renegotiate_to`]).
    pub fn renegotiate_to(&mut self, drn: u8) -> bool {
        self.v90.as_mut().is_some_and(|m| m.renegotiate_to(drn))
    }

    /// The rate menu, once a V.90 start-up has read its DIL (see
    /// [`analogue::Modem::rate_menu`]).
    pub fn rate_menu(&mut self) -> Option<analogue::RateMenu> {
        self.v90.as_mut()?.rate_menu()
    }

    /// Whether V.90 is what the call came to: PCM downstream, which V.92's
    /// PCM upstream has as well.
    pub fn is_v90(&self) -> bool {
        self.v90.is_some() || self.v92.is_some()
    }

    /// Whether the far end asked for PCM upstream (Table 18/V.92) in the
    /// phase 2 that has just finished, if one has.
    pub fn pcm_upstream_asked(&self) -> bool {
        self.v34.phase2().info1a_pcm_up().is_some()
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

    /// What V.90's phases 3 and 4 have done since this was last asked, a line
    /// each for the transcript (see [`analogue::Modem::take_notes`]), kept
    /// here so that a start-up that fails and is put away still gets its
    /// last lines told.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
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
        if self.v92.is_some() {
            // V.92's own rate renegotiation is not built.
            return false;
        }
        match self.v90.as_mut() {
            Some(m) => m.renegotiate(receive),
            None => self.v34.renegotiate((receive / 2400).clamp(1, 14) as u8),
        }
    }

    /// End the call from data mode (V.90 9.7, or V.34 11.7).
    pub fn clear_down(&mut self) -> bool {
        if self.v92.is_some() {
            // V.92's cleardown is a CPu of drn 0 (9.11), which nothing here
            // sends yet: the caller hangs up instead.
            return false;
        }
        match self.v90.as_mut() {
            Some(m) => m.clear_down(),
            None => self.v34.clear_down(),
        }
    }

    /// Back through V.90's phase 2.
    fn back_to_phase2(&mut self) {
        self.v92 = None;
        if let Some(m) = self.v90.take() {
            self.renegotiations += m.renegotiations();
            self.holes += m.holes();
        }
        self.v34.restart_phase2();
    }

    pub fn status(&self) -> Status {
        if let Some(m) = self.v92() {
            return match m.status() {
                pcm_up::Status::Connected { downstream, upstream } => {
                    Status::Connected { transmit: upstream, receive: downstream }
                }
                pcm_up::Status::ClearedDown => Status::ClearedDown,
                // A retrain or a failed Phase 3 goes back through phase 2 on
                // the sample it is seen (see `step`); until then the call is
                // what it was.
                _ if self.connected_once => Status::Retraining,
                _ => Status::Running,
            };
        }
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
        if let Some(m) = self.v92() {
            return m.phase();
        }
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
        if let Some(m) = self.v92.as_mut() {
            return m.modem_mut().take_bits();
        }
        match self.v90.as_mut() {
            Some(m) => m.take_bits(),
            None => self.v34.take_bits(),
        }
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        if let Some(m) = self.v92.as_mut() {
            m.modem_mut().send_bits(bits);
            return;
        }
        match self.v90.as_mut() {
            Some(m) => m.send_bits(bits),
            None => self.v34.send_bits(bits),
        }
    }

    pub fn pending_bits(&self) -> usize {
        if let Some(m) = self.v92() {
            return m.pending_bits();
        }
        match self.v90.as_ref() {
            Some(m) => m.pending_bits(),
            None => self.v34.pending_bits(),
        }
    }

    /// Whether there is anything to send bits into.
    pub fn accepts_bits(&self) -> bool {
        if let Some(m) = self.v92() {
            return matches!(m.status(), pcm_up::Status::Connected { .. });
        }
        match self.v90.as_ref() {
            Some(m) => matches!(m.status(), analogue::Status::Connected { .. }),
            None => self.v34.accepts_bits(),
        }
    }

    /// Whether the far end's data signal is there.
    pub fn carrier(&self) -> bool {
        if let Some(m) = self.v92() {
            // 109 comes on with B1d (9.6.2.1.6) and only a retrain, which
            // builds a new modem, turns it off; before data mode there is
            // nothing to lose.
            return m.circuit109() || !matches!(m.status(), pcm_up::Status::Connected { .. });
        }
        match self.v90.as_ref() {
            Some(m) => m.carrier(),
            None => self.v34.carrier(),
        }
    }

    /// Start a full retrain (9.5.2.1): back through V.90's phase 2. False if
    /// there is no call to retrain.
    pub fn retrain(&mut self) -> bool {
        if self.v92.is_some() {
            // 9.7.2.1: this end's own reasons, and phase 2 again.
            self.back_to_phase2();
            return true;
        }
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
        if let Some(m) = self.v92.as_mut() {
            let out = m.step(line);
            self.notes.extend(m.take_notes());
            match m.modem().status() {
                pcm_up::Status::Connected { .. } => {
                    self.connected_once = true;
                    self.failed_v92 = 0;
                }
                pcm_up::Status::Retrain(why) => {
                    // 9.7.2: Tone A and phase 2, whichever end began it.
                    self.notes.push(format!("V.92 retrain: {why}"));
                    self.back_to_phase2();
                }
                pcm_up::Status::Failed(why) => {
                    // 9.5.2.2: back to phase 2. Twice in a row, and the next
                    // INFO1a asks for V.90 mode instead (9.3).
                    self.last_failure = Some(why);
                    self.failed_v92 += 1;
                    self.back_to_phase2();
                    if self.failed_v92 >= V92_RETRAINS {
                        self.notes.push("V.90 mode next time round, not PCM upstream".into());
                        self.v34.decline_pcm_upstream();
                    }
                }
                _ => {}
            }
            return out;
        }
        if let Some(m) = self.v90.as_mut() {
            let out = m.step(line);
            self.notes.extend(m.take_notes());
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
                        self.notes.push("V.34 next time round, not V.90".into());
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
        // V.92 first: a Table 18 INFO1a is PCM upstream, and only one of the
        // two modems is ever built (9.5.2, 9.6.2).
        if p2.status() == phase2::Status::Done
            && self.v34.training().is_none()
            && let (Some(asked), Some(server)) = (p2.info1a_pcm_up(), p2.far_info0d())
        {
            // No DIL: the CPt is `readable_cpt`'s careful guess, and SCR
            // stands in for the DIL (9.5.2.1.10). Every upstream rate of 6.1
            // is offered, and the digital modem's CPd chooses.
            let mut settings = pcm_up::Settings::new(Descriptor::none(), 0x7ffff, pcm_up::readable_cpt());
            settings.md_length = asked.md_length;
            settings.uinfo = asked.uinfo;
            settings.limits = FilterLimits::from_info1a(asked.sections, asked.ltot_code, asked.lmax_code);
            settings.round_trip = p2.round_trip().unwrap_or(0.0);
            settings.law = if server.a_law { Law::A } else { Law::Mu };
            self.notes.push(format!(
                "V.92: PCM upstream (Table 18), U_INFO {}, {}-law",
                asked.uinfo,
                if server.a_law { "A" } else { "mu" }
            ));
            self.v92 = Some(Box::new(pcm_line::Line::new(settings, self.fs)));
            return out;
        }
        if p2.status() == phase2::Status::Done
            && self.v34.training().is_none()
            && let (Some(asked), Some(server), Some(info1d)) = (p2.info1a_pcm(), p2.far_info0d(), p2.info1c())
        {
            let ours_wide = true;
            let mut settings = analogue::Settings::new(&server, &info1d, &asked, p2.round_trip().unwrap_or(0.0), ours_wide);
            settings.v34_receive = p2.v34_receive_rate().unwrap_or(0);
            settings.pinned = self.pinned;
            // The call's first phase 2's, which every retrain's phase 2 is
            // handed on from (see [`phase2::Modem::again`]).
            settings.tone_b_level = p2.tone_b_level();
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
    /// V.92's Phase 3 and 4, when the analogue modem asked for PCM upstream
    /// with a Table 18 INFO1a (9.5, 9.6). At most one of this and `v90` is
    /// ever built.
    v92: Option<pcm::Modem>,
    /// What this end said in ITS INFO0d, and what it wants of V.92: phase 2
    /// is rebuilt from these when V.92 is asked for (see [`Self::with_v92`]).
    info0d: Info0d,
    lead_in: f64,
    wish: V92Wish,
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
    pub fn new(info0d: Info0d, lead_in: f64) -> Self {
        let law = if info0d.a_law { Law::A } else { Law::Mu };
        // A full-scale sine is +3.17 dBm0 in G.711.
        let phase2_gain = 10f64.powf((info0d.nominal_dbm0() - 3.17) / 20.0);
        Self {
            law,
            v34: v34::Modem::with_phase2(phase2::Modem::v90(Pcm::Digital(info0d), digital::FS, lead_in), digital::FS),
            v90: None,
            v92: None,
            info0d,
            lead_in,
            wish: V92Wish::default(),
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

    /// Offer V.92: INFO0d's two capability bits as Table 15/V.92 lays them
    /// out, and whether PCM upstream is wanted if the analogue modem asks
    /// for it with a Table 18 INFO1a.
    ///
    /// Phase 2 is rebuilt with V.92's INFO0 in it, so that the exchange is
    /// V.92's from the first INFO0d out. Everything after phase 2 follows
    /// what the analogue modem then asks for: a Table 10 INFO1a is V.90 and
    /// unchanged, a Table 18 one is PCM upstream, which is
    /// [`crate::v92::digital`]'s (9.5, 9.6).
    ///
    /// Call this before the call is placed; it throws the start-up away,
    /// which at that point is only the empty phase 2.
    pub fn with_v92(mut self, wish: V92Wish) -> Self {
        self.wish = wish;
        // The far ends this is for take a quarter of a second to turn round
        // to TRN1d, which the short one is over before (see `digital::TRN1D`).
        self.habits = digital::Habits::PATIENT;
        self.v34 = v34::Modem::with_phase2(
            phase2::Modem::v90(Pcm::DigitalV92(self.info0d, wish), digital::FS, self.lead_in),
            digital::FS,
        );
        self
    }

    /// What this end wants of V.92.
    pub fn wish(&self) -> V92Wish {
        self.wish
    }

    /// Whether both modems have shown V.92 capability (9.3).
    pub fn both_v92(&self) -> bool {
        self.v34.phase2().both_v92()
    }

    pub fn v34(&self) -> &v34::Modem {
        &self.v34
    }

    pub fn v90(&self) -> Option<&digital::Modem> {
        self.v90.as_ref()
    }

    /// V.92's Phase 3 and 4, once PCM upstream is what the call came to.
    pub fn v92(&self) -> Option<&pcm::Modem> {
        self.v92.as_ref()
    }

    /// Whether V.90 is what the call came to.
    pub fn is_v90(&self) -> bool {
        self.v90.is_some() || self.v92.is_some()
    }

    /// Whether V.92's PCM upstream is what the call came to.
    pub fn is_v92(&self) -> bool {
        self.v92.is_some()
    }

    /// What V.92's Phases 3 and 4 have done since this was last asked, a
    /// line each for the transcript.
    pub fn take_notes(&mut self) -> Vec<String> {
        self.v92.as_mut().map(pcm::Modem::take_notes).unwrap_or_default()
    }

    pub fn round_trip(&self) -> Option<f64> {
        self.v34.phase2().round_trip()
    }

    /// Whether there is anything to send bits into.
    pub fn accepts_bits(&self) -> bool {
        if self.v92.is_some() {
            // PCM inbound data mode is not built yet: Phase 4 stops at TRN2d
            // (see [`pcm::Modem`]), and nothing hands bits over before it.
            return matches!(self.v92.as_ref().map(pcm::Modem::status), Some(pcm::Status::Connected { .. }));
        }
        match self.v90.as_ref() {
            Some(m) => matches!(m.status(), digital::Status::Connected { .. }),
            None => self.v34.accepts_bits(),
        }
    }

    /// Whether the far end's data signal is there.
    pub fn carrier(&self) -> bool {
        if let Some(m) = self.v92.as_ref() {
            return !matches!(m.status(), pcm::Status::Failed(_) | pcm::Status::ClearedDown);
        }
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
        if self.v92.is_some() {
            return None;
        }
        match self.v90.as_ref() {
            Some(m) => m.last_point(),
            None => self.v34.constellation_point(),
        }
    }

    /// The largest coordinate the upstream's points reach, in the units of
    /// [`Self::constellation_point`].
    pub fn constellation_peak(&self) -> Option<f64> {
        if self.v92.is_some() {
            return None;
        }
        match self.v90.as_ref() {
            Some(m) => Some(m.upstream_peak()),
            None => self.v34.constellation_peak(),
        }
    }

    /// Points the upstream is read against.
    pub fn constellation_size(&self) -> Option<usize> {
        if self.v92.is_some() {
            return None;
        }
        match self.v90.as_ref() {
            Some(m) => Some(m.upstream_points()),
            None => self.v34.constellation_size(),
        }
    }

    pub fn status(&self) -> Status {
        if let Some(m) = self.v92.as_ref() {
            return match m.status() {
                pcm::Status::Running | pcm::Status::Phase4 if self.connected_once => Status::Retraining,
                pcm::Status::Running | pcm::Status::Phase4 => Status::Running,
                pcm::Status::Failed(why) => Status::Failed(why),
                pcm::Status::ClearedDown => Status::ClearedDown,
                pcm::Status::Connected { downstream, upstream } => Status::Connected { transmit: downstream, receive: upstream },
            };
        }
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
        if let Some(m) = self.v92.as_ref() {
            return m.phase();
        }
        match self.v90.as_ref() {
            Some(m) => m.phase(),
            None if self.v34.training().is_none() => "V.90 phase 2",
            None => self.v34.phase(),
        }
    }

    pub fn take_bits(&mut self) -> Vec<bool> {
        if let Some(m) = self.v92.as_mut() {
            return m.take_bits();
        }
        match self.v90.as_mut() {
            Some(m) => m.take_bits(),
            None => self.v34.take_bits(),
        }
    }

    pub fn send_bits(&mut self, bits: &[bool]) {
        if let Some(m) = self.v92.as_mut() {
            m.send_bits(bits);
            return;
        }
        match self.v90.as_mut() {
            Some(m) => m.send_bits(bits),
            None => self.v34.send_bits(bits),
        }
    }

    pub fn pending_bits(&self) -> usize {
        if let Some(m) = self.v92.as_ref() {
            return m.pending_bits();
        }
        match self.v90.as_ref() {
            Some(m) => m.pending_bits(),
            None => self.v34.pending_bits(),
        }
    }

    /// How many bits one downstream fill can take, for a caller keeping the
    /// queue fed (see [`pcm::Modem::fill_bits`]). Zero when there is no V.92
    /// data mode, whose fills are the ones that take more than a handful.
    pub fn fill_bits(&self) -> usize {
        self.v92.as_ref().map_or(0, pcm::Modem::fill_bits)
    }

    /// Start a full retrain (9.5.1.1).
    pub fn retrain(&mut self) -> bool {
        if self.v92.is_some() {
            // 9.7.1.1: back through V.92's Phase 2, which for this end is
            // V.34's phase 2 with V.90's INFO sequences in it.
            self.v92 = None;
            self.v34.restart_phase2();
            return true;
        }
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
        if self.v92.is_some() {
            return false;
        }
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
        if let Some(m) = self.v92.as_mut() {
            let out = m.step(input);
            let failed = matches!(m.status(), pcm::Status::Failed(_));
            if matches!(m.status(), pcm::Status::Connected { .. }) {
                self.failed_starts = 0;
                self.connected_once = true;
            }
            if failed && self.failed_starts < V90_RETRAINS {
                // 9.7.1.1: back through Phase 2 for another go.
                self.failed_starts += 1;
                self.v92 = None;
                self.v34.restart_phase2();
            }
            return out;
        }
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
        if std::env::var_os("PHASE2_TRACE").is_some() && p2.status() == phase2::Status::Done {
            eprintln!(
                "P2TRACE data mode: both_v92 {} far_flags {:?} table18 {:?} table10 {:?}",
                p2.both_v92(),
                p2.far_pcm_flags(),
                p2.info1a_pcm_up().is_some(),
                p2.info1a_pcm().is_some()
            );
        }
        // V.92 first: a Table 18 INFO1a is PCM upstream, and only one of the
        // two modems is ever built (9.5, 9.6).
        if p2.status() == phase2::Status::Done
            && self.v34.training().is_none()
            && let Some(asked) = p2.info1a_pcm_up()
        {
            let wide = p2.far_capabilities().is_some_and(|f| f.constellation_1664);
            let mut settings = pcm::Settings::new(self.law, &asked, p2.round_trip().unwrap_or(0.0), wide);
            settings.habits = pcm::Habits::LIVE;
            self.v92 = Some(pcm::Modem::new(settings));
        }
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
