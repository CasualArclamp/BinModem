//! What a full start-up taught about this connection (1 j, 9.2, 9.4): enough
//! to ask for a short Phase 1 and a short Phase 2 the next time the same
//! number answers.
//!
//! V.92 calls it "reduced start-up time on recognized connections" and says
//! nothing about what recognising one means. Here it means this: a V.92 call
//! to the same dial string reached data mode before, and what that call
//! measured is still in hand. The modem crate keeps one of these for the
//! number being dialled and whoever owns the modem stores it between runs,
//! as a line of text so that a bad one can be read and deleted by hand.
//!
//! It holds only what the short procedures cannot find out for themselves.
//! Short Phase 1 has the analogue modem name a Ucode for QTS before anything
//! has been measured (Table 2). Short Phase 2 leaves out the line probe and
//! INFO1d, and its one ranging exchange gives the round trip to the digital
//! modem alone (9.4.1.1.4), so the analogue modem's comes from here; and an
//! analogue modem may ask for it only if it means to connect in a PCM mode
//! (9.4), which the last call is the evidence for.

use v8::quick::{AnspcmLevel, Uqts};

/// The Ucode QTS is asked for at when nothing better is known: Table 2's
/// `0101`, about -18 dBm0 as a tone, well inside what a softphone's limiter
/// passes.
pub const DEFAULT_UQTS: u8 = 70;

/// The level ANSpcm goes at when this end is the one naming it: Table 13's
/// `01`, -12 dBm0.
pub const DEFAULT_LEVEL: AnspcmLevel = AnspcmLevel::Minus12;

/// What a call that reached V.92's data mode taught about the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recognized {
    /// The Ucode to ask for QTS at in QC1a (Table 2).
    pub uqts: u8,
    /// Whether the digital modem said A-law in its INFO0d.
    pub a_law: bool,
    /// The round-trip delay Phase 2 measured, in milliseconds.
    pub round_trip_ms: u32,
    /// Whether the call's upstream was PCM (a Table 18 INFO1a), which is
    /// what a short Phase 2 would ask for again.
    pub pcm_upstream: bool,
}

impl Recognized {
    /// A connection nothing is known about beyond that V.92 is to be tried
    /// the short way: for an owner that has been told to force it.
    pub fn unmeasured() -> Self {
        Self { uqts: DEFAULT_UQTS, a_law: false, round_trip_ms: 0, pcm_upstream: false }
    }

    /// The U_QTS field of the QC1a this asks for.
    pub fn qts(&self) -> Uqts {
        Uqts::from_ucode(self.uqts)
            .or_else(|| Uqts::from_ucode(DEFAULT_UQTS))
            .expect("the default is one of Table 2's")
    }

    /// One line of text, for a file a person can read.
    pub fn to_text(&self) -> String {
        format!(
            "uqts={} law={} rtd={} pcm_up={}",
            self.uqts,
            if self.a_law { "a" } else { "mu" },
            self.round_trip_ms,
            u8::from(self.pcm_upstream)
        )
    }

    /// Read [`Self::to_text`] back. Fields may come in any order; one that
    /// is missing or will not parse, or a U_QTS that is not one of Table
    /// 2's, makes the whole line nothing -- a memo that cannot be trusted is
    /// a full start-up, which always works.
    pub fn from_text(text: &str) -> Option<Self> {
        let (mut uqts, mut a_law, mut rtd, mut pcm_up) = (None, None, None, None);
        for field in text.split_whitespace() {
            let (key, value) = field.split_once('=')?;
            match key {
                "uqts" => uqts = Some(value.parse::<u8>().ok()?),
                "law" => {
                    a_law = Some(match value {
                        "a" => true,
                        "mu" => false,
                        _ => return None,
                    });
                }
                "rtd" => rtd = Some(value.parse::<u32>().ok()?),
                "pcm_up" => {
                    pcm_up = Some(match value {
                        "1" => true,
                        "0" => false,
                        _ => return None,
                    });
                }
                // A field a later version added: not this one's to judge.
                _ => {}
            }
        }
        let uqts = uqts?;
        Uqts::from_ucode(uqts)?.ucode()?;
        Some(Self { uqts, a_law: a_law?, round_trip_ms: rtd?, pcm_upstream: pcm_up? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_memo_round_trips_through_its_text_form() {
        for memo in [
            Recognized::unmeasured(),
            Recognized { uqts: 79, a_law: true, round_trip_ms: 412, pcm_upstream: true },
        ] {
            assert_eq!(Recognized::from_text(&memo.to_text()), Some(memo), "{}", memo.to_text());
        }
    }

    #[test]
    fn a_memo_that_cannot_be_trusted_is_no_memo() {
        // Ucode 64 is not in Table 2; a missing field; a law nobody has.
        assert_eq!(Recognized::from_text("uqts=64 law=mu rtd=10 pcm_up=1"), None);
        assert_eq!(Recognized::from_text("uqts=70 law=mu pcm_up=1"), None);
        assert_eq!(Recognized::from_text("uqts=70 law=x rtd=10 pcm_up=1"), None);
        assert_eq!(Recognized::from_text(""), None);
        // An unknown field is passed over.
        assert!(Recognized::from_text("uqts=70 law=mu rtd=10 pcm_up=0 later=7").is_some());
    }
}
