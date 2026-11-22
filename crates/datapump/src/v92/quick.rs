//! Short Phase 1 as the digital modem sends it: what follows QCA1d (9.2.4.1,
//! 9.2.4.3).
//!
//! V.8's part of a quick connect is two sequences on V.21, and `crate::v8`
//! has them. What is here is the rest of the answering side: 75 ms of
//! silence, QTS, QTS-bar and ANSpcm, with an ear for TONEq. These are PCM
//! signals -- codewords, one to a network sample -- so they belong to the
//! digital modem's pump and run at its 8000 Hz, and a line that carries
//! G.711 itself gets them as the octets [`super::anspcm`] defines.
//!
//! The analogue side of the same exchange is tones at the line's rate and
//! lives in `crate::v8`, where the way back to CM already is.

use v8::quick::AnspcmLevel;

use crate::v90::ucode::{self, Law};

use super::anspcm::{self, ToneqDetector};

/// The network's sample rate.
const FS: f64 = 8000.0;

/// "Silence for 75 +/- 5 ms" after QCA1d (9.2.4.1), in network samples.
pub const SILENCE: u64 = 600;

/// "If TONEq is not detected during the 2 s following transmission of QCA1d,
/// the digital modem shall transmit ANSam and proceed according to ITU-T
/// V.8" (9.2.4.3), in network samples.
pub const PATIENCE: u64 = 16_000;

/// How the answer is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Silence, QTS, QTS-bar or ANSpcm, and no TONEq yet.
    Running,
    /// TONEq was heard: 75 ms of silence and Phase 2 are next.
    Answered,
    /// Two seconds and no TONEq: back to ANSam and V.8.
    GaveUp,
}

/// The digital modem from the end of its QCA1d to the TONEq that answers it.
#[derive(Debug, Clone)]
pub struct Answer {
    law: Law,
    /// The Ucode QTS is built on, which the analogue modem asked for in its
    /// QC1a (Table 2).
    uqts: u8,
    /// The level ANSpcm goes at, which this end's QCA1d named (Table 13).
    level: AnspcmLevel,
    /// Network samples since QCA1d ended.
    n: u64,
    toneq: ToneqDetector,
    status: Status,
}

impl Answer {
    pub fn new(law: Law, uqts: u8, level: AnspcmLevel) -> Self {
        Self {
            law,
            uqts,
            level,
            n: 0,
            toneq: ToneqDetector::new(FS),
            status: Status::Running,
        }
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// What is on the line, for a scope.
    pub fn phase(&self) -> &'static str {
        match self.n {
            n if n < SILENCE => "QCA1d silence",
            n if n < SILENCE + anspcm::QTS_SYMBOLS as u64 => "QTS",
            n if n < SILENCE + anspcm::QTS_TOTAL as u64 => "QTS-bar",
            _ => "ANSpcm",
        }
    }

    /// The octet for network sample `n`, counted from the end of QCA1d.
    pub fn octet(&self, n: u64) -> u8 {
        let Some(n) = n.checked_sub(SILENCE) else {
            return anspcm::silence_octet(self.law);
        };
        // The first QTS symbol is data frame interval 0 (8.3.6), and ANSpcm
        // follows QTS-bar with nothing between them.
        anspcm::qts_octet(self.law, self.uqts, n).unwrap_or_else(|| {
            anspcm::anspcm_octet(self.level, self.law, n - anspcm::QTS_TOTAL as u64)
        })
    }

    /// One network sample in, one level out: the codeword's own amplitude,
    /// full scale being one, as every level the digital modem sends is.
    pub fn step(&mut self, input: f64) -> f64 {
        if self.status != Status::Running {
            return level(self.law, anspcm::silence_octet(self.law));
        }
        let n = self.n;
        self.n += 1;
        // "While ANSpcm is transmitted, the modem shall condition its
        // receiver to detect TONEq" (9.2.4.3). The ear is open from the
        // start so that it has settled by then; what it latches before
        // ANSpcm is on the line was not an answer to it.
        self.toneq.feed(input);
        let anspcm_on = n >= SILENCE + anspcm::QTS_TOTAL as u64;
        if !anspcm_on && self.toneq.heard() {
            self.toneq.reset();
        }
        if anspcm_on && self.toneq.heard() {
            self.status = Status::Answered;
        } else if n >= PATIENCE {
            self.status = Status::GaveUp;
        }
        level(self.law, self.octet(n))
    }
}

/// An octet's amplitude as the codec would put it out, full scale being one.
pub fn level(law: Law, octet: u8) -> f64 {
    let (u, negative) = ucode::from_octet(law, octet);
    let l = ucode::level(law, u);
    if negative { -l } else { l }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v92::anspcm::ToneqGenerator;

    /// The octets are the Recommendation's: silence, then QTS from frame
    /// interval 0, QTS-bar at 768 and ANSpcm at 816.
    #[test]
    fn the_answer_is_silence_then_qts_then_anspcm() {
        let a = Answer::new(Law::Mu, 79, AnspcmLevel::Minus12);
        assert_eq!(a.octet(0), anspcm::silence_octet(Law::Mu));
        assert_eq!(a.octet(SILENCE - 1), anspcm::silence_octet(Law::Mu));
        for n in 0..anspcm::QTS_TOTAL as u64 {
            assert_eq!(
                Some(a.octet(SILENCE + n)),
                anspcm::qts_octet(Law::Mu, 79, n),
                "QTS symbol {n}"
            );
        }
        for n in 0..700 {
            assert_eq!(
                a.octet(SILENCE + anspcm::QTS_TOTAL as u64 + n),
                anspcm::anspcm_octet(AnspcmLevel::Minus12, Law::Mu, n),
                "ANSpcm symbol {n}"
            );
        }
    }

    /// TONEq while ANSpcm is on the line ends it; TONEq before does not.
    #[test]
    fn toneq_answers_anspcm_and_nothing_earlier() {
        let mut a = Answer::new(Law::Mu, 79, AnspcmLevel::Minus12);
        let mut tone = ToneqGenerator::new(FS);
        // A mark held through the silence and QTS: a caller still idling its
        // V.21 channel, say.
        for _ in 0..SILENCE + anspcm::QTS_TOTAL as u64 {
            a.step(tone.step());
            assert_eq!(a.status(), Status::Running);
        }
        for _ in 0..200 {
            a.step(0.0);
        }
        assert_eq!(a.status(), Status::Running);
        let mut took = 0;
        while a.status() == Status::Running && took < 2000 {
            a.step(tone.step());
            took += 1;
        }
        assert_eq!(a.status(), Status::Answered);
        assert!(took < 1000, "TONEq took {took} samples to hear");
    }

    /// 9.2.4.3: two seconds after QCA1d with no TONEq, back to V.8.
    #[test]
    fn no_toneq_in_two_seconds_gives_up() {
        let mut a = Answer::new(Law::Mu, 79, AnspcmLevel::Minus12);
        for _ in 0..PATIENCE {
            a.step(0.0);
            assert_eq!(a.status(), Status::Running);
        }
        a.step(0.0);
        assert_eq!(a.status(), Status::GaveUp);
    }
}
