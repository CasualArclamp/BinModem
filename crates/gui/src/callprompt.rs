//! A call arriving on a SIP line: ring, and ask what to answer it as.
//!
//! Nothing here answers by itself. A call ringing here puts a question in the
//! middle of the window -- answer as a modem, answer as a fax, or decline --
//! and rings until one of them is chosen or the caller gives up.

use eframe::egui;
use sip::ua::state;

/// What the person chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// A data call, in whatever mode the modem is set to.
    Modem,
    /// T.30, receiving: what "Wait for a fax" does.
    Fax,
    /// 603 Decline.
    Decline,
}

/// Where the question has got to.
#[derive(Debug, Clone, PartialEq)]
pub enum Prompt {
    /// No call ringing here.
    Quiet,
    /// A call is ringing here and nobody has said what to do with it.
    Ringing { caller: String, since: f64 },
    /// Answered or declined, and the line has not caught up yet: it goes on
    /// saying "ringing here" for the few milliseconds the agent takes, and
    /// the question must not come back in that gap.
    Handled { caller: String, at: f64 },
}

/// How a call that rang here stopped ringing without being chosen for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The caller hung up first (CANCEL), or the call was declined elsewhere.
    Missed(String),
    /// Answered some other way: ATA typed at the terminal.
    AnsweredElsewhere,
}

/// How long a choice may sit with the line still ringing here before the
/// question comes back: long enough for any answer to take, short enough
/// that a choice that went nowhere does not leave a call ringing unseen.
const HANDLED_FOR: f64 = 5.0;

#[derive(Debug, Clone, PartialEq)]
pub struct CallPrompt {
    pub prompt: Prompt,
}

impl Default for CallPrompt {
    fn default() -> Self {
        Self { prompt: Prompt::Quiet }
    }
}

impl CallPrompt {
    /// What the SIP line says, each frame. `call` is its call state, None for
    /// a line that is not SIP or not open. Returns how a call ended, when one
    /// that was ringing stopped without a choice.
    pub fn observe(&mut self, call: Option<&str>, caller: Option<&str>, now: f64) -> Option<Ended> {
        let here = call == Some(state::RINGING_HERE);
        let named = || caller.filter(|c| !c.is_empty()).unwrap_or("someone").to_owned();
        match &mut self.prompt {
            Prompt::Quiet if here => {
                self.prompt = Prompt::Ringing { caller: named(), since: now };
                None
            }
            Prompt::Quiet => None,
            Prompt::Ringing { caller: who, .. } if here => {
                if let Some(c) = caller.filter(|c| !c.is_empty()) {
                    c.clone_into(who);
                }
                None
            }
            Prompt::Ringing { caller: who, .. } => {
                let who = std::mem::take(who);
                self.prompt = Prompt::Quiet;
                Some(if call == Some(state::ANSWERED) {
                    Ended::AnsweredElsewhere
                } else {
                    Ended::Missed(who)
                })
            }
            Prompt::Handled { caller: who, at } if here => {
                if now - *at > HANDLED_FOR {
                    let who = std::mem::take(who);
                    self.prompt = Prompt::Ringing { caller: who, since: now };
                }
                None
            }
            Prompt::Handled { .. } => {
                self.prompt = Prompt::Quiet;
                None
            }
        }
    }

    /// A button pressed. The choice comes back only while the call is still
    /// ringing, so a second press, or one after the caller gave up, does
    /// nothing.
    pub fn choose(&mut self, choice: Choice, now: f64) -> Option<Choice> {
        let Prompt::Ringing { caller, .. } = &mut self.prompt else {
            return None;
        };
        let caller = std::mem::take(caller);
        self.prompt = Prompt::Handled { caller, at: now };
        Some(choice)
    }

    /// Who is calling and since when, while the call rings and nobody has
    /// chosen.
    pub fn ringing(&self) -> Option<(&str, f64)> {
        match &self.prompt {
            Prompt::Ringing { caller, since } => Some((caller.as_str(), *since)),
            _ => None,
        }
    }
}

/// A telephone's ring, for the monitor: 400 and 450 Hz together, in the
/// double ring of Australia and Britain -- 0.4 s on, 0.2 off, 0.4 on, 2 off.
#[derive(Debug, Clone)]
pub struct Ringer {
    rate: f64,
    /// Samples made since the ringing started.
    made: u64,
    /// The window's clock when samples were last made.
    last: Option<f64>,
}

/// The cadence, in seconds: on, off, on, off.
const CADENCE: [f64; 4] = [0.4, 0.2, 0.4, 2.0];

/// The most made at once: a window that has not drawn for a while does not
/// get the whole gap at once, which would only overflow the monitor.
const MOST_AT_ONCE: f64 = 0.2;

impl Ringer {
    pub fn new(rate: f64) -> Self {
        Self { rate, made: 0, last: None }
    }

    /// Whether the bell is sounding `t` seconds into the ringing.
    pub fn sounding(t: f64) -> bool {
        let cycle: f64 = CADENCE.iter().sum();
        let t = t.rem_euclid(cycle);
        t < CADENCE[0] || (CADENCE[0] + CADENCE[1]..CADENCE[0] + CADENCE[1] + CADENCE[2]).contains(&t)
    }

    /// The sample at `t` seconds in.
    pub fn sample(t: f64) -> f32 {
        if !Self::sounding(t) {
            return 0.0;
        }
        let tau = std::f64::consts::TAU;
        (0.2 * ((tau * 400.0 * t).sin() + (tau * 450.0 * t).sin())) as f32
    }

    /// The samples owed since the last call, by the window's clock.
    pub fn fill(&mut self, now: f64, out: &mut Vec<f32>) {
        let since = self.last.map_or(MOST_AT_ONCE, |last| (now - last).clamp(0.0, MOST_AT_ONCE));
        self.last = Some(now);
        let n = (since * self.rate).round() as u64;
        out.extend((self.made..self.made + n).map(|i| Self::sample(i as f64 / self.rate)));
        self.made += n;
    }
}

/// The question, in the middle of the window over everything else: who is
/// calling, for how long, and the three answers. Its edge flashes with the
/// bell. Returns the button pressed, if one was.
///
/// Clicking outside it or pressing Escape does nothing: a call is answered
/// or declined, not dismissed and left ringing.
pub fn dialog(ctx: &egui::Context, caller: &str, ringing_for: f64, fax_class: bool) -> Option<Choice> {
    use egui::{Color32, RichText};
    let lit = Ringer::sounding(ringing_for);
    let edge = if lit { Color32::from_rgb(90, 220, 130) } else { Color32::from_rgb(58, 62, 72) };
    let frame = egui::Frame::popup(&ctx.global_style())
        .stroke(egui::Stroke::new(3.0, edge))
        .inner_margin(egui::Margin::same(20));
    egui::Modal::new(egui::Id::new("incoming-call"))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            let mut chosen = None;
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(if lit { "Ringing" } else { "Incoming call" })
                        .size(15.0)
                        .color(Color32::from_rgb(140, 150, 165)),
                );
                ui.add_space(4.0);
                ui.label(RichText::new(caller).monospace().size(26.0).strong().color(Color32::from_rgb(235, 240, 245)));
                ui.label(
                    RichText::new(format!("ringing for {:.0} s", ringing_for.max(0.0)))
                        .small()
                        .color(Color32::from_rgb(140, 150, 165)),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let size = egui::vec2(126.0, 38.0);
                    let modem_tip = if fax_class {
                        "A data call: AT+FCLASS=0 to make the modem a modem again, then ATA in \
                         the mode the mode box has set"
                    } else {
                        "A data call: ATA, in the mode the mode box has set"
                    };
                    if ui.add(egui::Button::new("Answer as modem").min_size(size)).on_hover_text(modem_tip).clicked() {
                        chosen = Some(Choice::Modem);
                    }
                    if ui
                        .add(egui::Button::new("Answer as fax").min_size(size))
                        .on_hover_text(
                            "T.30, receiving: AT+FCLASS=1 then ATA, as Wait for a fax does, with \
                             the Fax tab's settings. The page is drawn there as it arrives",
                        )
                        .clicked()
                    {
                        chosen = Some(Choice::Fax);
                    }
                    if ui
                        .add(egui::Button::new("Decline").min_size(size).fill(Color32::from_rgb(112, 46, 42)))
                        .on_hover_text("Turn the call away: 603 Decline")
                        .clicked()
                    {
                        chosen = Some(Choice::Decline);
                    }
                });
            });
            chosen
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ringing(prompt: &mut CallPrompt, caller: &str, now: f64) -> Option<Ended> {
        prompt.observe(Some(state::RINGING_HERE), Some(caller), now)
    }

    #[test]
    fn a_call_ringing_here_asks_and_nothing_answers_by_itself() {
        let mut p = CallPrompt::default();
        assert_eq!(p.observe(Some(state::IDLE), None, 0.0), None);
        assert_eq!(p.ringing(), None);
        ringing(&mut p, "0412345678", 1.0);
        assert_eq!(p.ringing(), Some(("0412345678", 1.0)));
        // A minute of ringing is a minute of asking.
        for t in 2..60 {
            ringing(&mut p, "0412345678", f64::from(t));
        }
        assert_eq!(p.ringing(), Some(("0412345678", 1.0)));
    }

    #[test]
    fn each_choice_comes_back_once_and_the_question_does_not_return_while_the_line_catches_up() {
        for choice in [Choice::Modem, Choice::Fax, Choice::Decline] {
            let mut p = CallPrompt::default();
            ringing(&mut p, "0412345678", 0.0);
            assert_eq!(p.choose(choice, 1.0), Some(choice));
            assert_eq!(p.choose(choice, 1.1), None, "a second press");
            // Still ringing here for a moment: no question, no ring.
            ringing(&mut p, "0412345678", 1.2);
            assert_eq!(p.ringing(), None);
            let next = if choice == Choice::Decline { state::IDLE } else { state::ANSWERED };
            assert_eq!(p.observe(Some(next), None, 1.3), None);
            assert_eq!(p.prompt, Prompt::Quiet);
        }
    }

    #[test]
    fn a_caller_who_hangs_up_first_stops_the_ringing_and_is_a_missed_call() {
        let mut p = CallPrompt::default();
        ringing(&mut p, "0412345678", 0.0);
        assert_eq!(p.observe(Some(state::IDLE), None, 4.0), Some(Ended::Missed("0412345678".to_owned())));
        assert_eq!(p.ringing(), None);
        assert_eq!(p.choose(Choice::Fax, 4.1), None, "a press after the caller gave up");
    }

    #[test]
    fn a_call_answered_at_the_terminal_is_not_missed() {
        let mut p = CallPrompt::default();
        ringing(&mut p, "0412345678", 0.0);
        assert_eq!(p.observe(Some(state::ANSWERED), None, 2.0), Some(Ended::AnsweredElsewhere));
    }

    #[test]
    fn a_choice_that_went_nowhere_asks_again() {
        let mut p = CallPrompt::default();
        ringing(&mut p, "0412345678", 0.0);
        p.choose(Choice::Modem, 1.0);
        ringing(&mut p, "0412345678", 3.0);
        assert_eq!(p.ringing(), None);
        ringing(&mut p, "0412345678", 7.0);
        assert_eq!(p.ringing().map(|(c, _)| c), Some("0412345678"));
    }

    #[test]
    fn a_caller_with_no_name_is_someone_and_a_name_arriving_later_is_kept() {
        let mut p = CallPrompt::default();
        p.observe(Some(state::RINGING_HERE), None, 0.0);
        assert_eq!(p.ringing().map(|(c, _)| c), Some("someone"));
        ringing(&mut p, "Front desk <0412345678>", 0.1);
        assert_eq!(p.ringing().map(|(c, _)| c), Some("Front desk <0412345678>"));
    }

    #[test]
    fn the_ring_is_a_double_ring() {
        assert!(Ringer::sounding(0.1));
        assert!(!Ringer::sounding(0.5));
        assert!(Ringer::sounding(0.7));
        assert!(!Ringer::sounding(1.5));
        assert!(Ringer::sounding(3.1), "the next cycle");
        assert_eq!(Ringer::sample(1.5), 0.0);
        let loudest = (0..8000).map(|i| Ringer::sample(f64::from(i) / 8000.0).abs()).fold(0.0, f32::max);
        assert!(loudest > 0.2 && loudest <= 0.4, "{loudest}");
    }

    #[test]
    fn the_ringer_makes_as_many_samples_as_time_has_passed() {
        let mut r = Ringer::new(8000.0);
        let mut out = Vec::new();
        r.fill(10.0, &mut out);
        assert_eq!(out.len(), 1600, "a first call makes a little to start the monitor");
        r.fill(10.05, &mut out);
        assert_eq!(out.len(), 2000);
        r.fill(30.0, &mut out);
        assert_eq!(out.len(), 3600, "a long gap is capped");
        r.fill(29.0, &mut out);
        assert_eq!(out.len(), 3600, "a clock going backwards makes nothing");
    }
}
