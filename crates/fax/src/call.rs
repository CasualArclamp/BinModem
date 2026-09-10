//! Phase A and B of a fax call, from the calling end.
//!
//! T.30 divides a call into five phases: A is getting the two machines to
//! agree they are faxes, B is finding out what they can do and settling on
//! it, C is the page, D is what to do next, E is hanging up. This is A and B,
//! and the beginning of E, because those are the parts that need no
//! modulation faster than 300 bit/s.
//!
//! Which is not a small part of a fax call. Everything a fax knows about the
//! machine at the other end it learns here, and every fax call that fails
//! before a page is sent fails here.

use crate::frames::{Message, Reader, Sender};
use crate::t30::{self, Capabilities, Frame};

/// How far the call has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Sending the calling tone and waiting for something to answer (5.1.1).
    Calling,
    /// Something answered. Waiting for it to say what it is.
    Listening,
    /// It said. Everything below is known.
    Heard,
    /// Our own identification and command are going out.
    Answering,
    /// Sending the disconnect that ends the call politely (5.3.7).
    Ending,
    /// Nothing more to do.
    Done,
    /// Nothing recognisable arrived in time.
    Failed,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Calling => "calling",
            Self::Listening => "listening",
            Self::Heard => "heard it",
            Self::Answering => "answering",
            Self::Ending => "hanging up",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// T1: how long to wait in phase B before giving up (5.3.3.1).
///
/// "35 s +/- 5 s", which is generous and meant to be: a machine at the other
/// end may be picking up paper, and the whole of phase B can pass before it
/// says anything at all.
pub const T1_SECONDS: f64 = 35.0;

/// A fax call, from the end that dialled.
#[derive(Debug)]
pub struct Call {
    phase: Phase,
    reader: Reader,
    sender: Sender,
    /// Seconds since the call began, which is what T1 counts.
    elapsed: f64,
    step: f64,
    /// What the far end said, as it says it.
    pub identity: String,
    pub capabilities: Option<Capabilities>,
    /// The capability field exactly as it arrived, so that anything wanting
    /// to read it differently still can.
    pub capability_field: Option<Vec<u8>>,
    /// Every frame either end sent, for the log.
    heard: Vec<Message>,
    /// Our own identification, sent as a TSI. Twenty characters, and T.30
    /// only allows digits, spaces and a plus.
    identification: String,
}

impl Call {
    pub fn new(fs: f64, identification: &str) -> Self {
        Self {
            phase: Phase::Calling,
            reader: Reader::new(),
            sender: Sender::new(),
            elapsed: 0.0,
            step: 1.0 / fs,
            identity: String::new(),
            capabilities: None,
            capability_field: None,
            heard: Vec::new(),
            identification: identification.to_owned(),
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn seconds(&self) -> f64 {
        self.elapsed
    }

    /// Frames seen since this was last asked, for a log.
    pub fn take_heard(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.heard)
    }

    /// Whether the calling tone should be going out just now.
    ///
    /// 5.1.1 has it on for half a second in every three and a half, and only
    /// until something answers. It is a courtesy rather than a requirement:
    /// it tells a person who picked up that a fax is waiting, and it tells a
    /// machine in automatic answer which of the two it is talking to.
    pub fn wants_calling_tone(&self) -> bool {
        if self.phase != Phase::Calling {
            return false;
        }
        let period = crate::CNG_ON + crate::CNG_OFF;
        self.elapsed % period < crate::CNG_ON
    }

    /// Whether we should be sending on V.21 just now.
    pub fn wants_carrier(&self) -> bool {
        matches!(self.phase, Phase::Answering | Phase::Ending)
            && !self.sender.is_empty()
    }

    /// The next bit to put on V.21, if any.
    pub fn next_bit(&mut self) -> Option<bool> {
        self.sender.next_bit()
    }

    /// One sample of time passing, with whatever V.21 recovered from it.
    ///
    /// `line_idle` says whether everything handed over has actually gone out.
    /// It is not the same question as whether this has any bits left: a
    /// transmitter is fed ahead of the line, so the last frame of a burst is
    /// still being modulated long after its last bit was handed over. Reading
    /// the two as one hung the call up before its own disconnect had reached
    /// the far end, which is exactly the rudeness the disconnect exists to
    /// avoid.
    pub fn advance(&mut self, bit: Option<bool>, line_idle: bool) {
        self.elapsed += self.step;
        if let Some(bit) = bit
            && let Some(message) = self.reader.feed(bit)
        {
            self.received(message);
        }
        match self.phase {
            Phase::Calling | Phase::Listening => {
                if self.elapsed > T1_SECONDS {
                    self.phase = Phase::Failed;
                }
            }
            Phase::Heard => self.reply(),
            Phase::Answering | Phase::Ending if self.sender.is_empty() && line_idle => {
                self.phase = if self.phase == Phase::Ending {
                    Phase::Done
                } else {
                    Phase::Ending
                };
                if self.phase == Phase::Ending {
                    self.hang_up();
                }
            }
            _ => {}
        }
    }

    fn received(&mut self, message: Message) {
        match message.frame {
            // Any frame at all means something down there is a fax.
            Frame::Csi | Frame::Nsf => {
                if message.frame == Frame::Csi {
                    self.identity = t30::identification(&message.fif);
                }
                if self.phase == Phase::Calling {
                    self.phase = Phase::Listening;
                }
            }
            Frame::Dis => {
                self.capabilities = Some(t30::capabilities(&message.fif));
                self.capability_field = Some(message.fif.clone());
                self.phase = Phase::Heard;
            }
            // 5.3.7: the far end may end the call at any point, and a
            // disconnect needs no answer.
            Frame::Dcn => self.phase = Phase::Done,
            _ => {
                if self.phase == Phase::Calling {
                    self.phase = Phase::Listening;
                }
            }
        }
        self.heard.push(message);
    }

    /// Say who we are, and then say goodbye.
    ///
    /// A page would go here. Until there is a modulation to carry one, the
    /// polite thing is to identify ourselves and disconnect rather than fall
    /// silent: a fax left waiting holds the line for its full T1 and then
    /// reports a failed receive to whoever is standing at it.
    fn reply(&mut self) {
        let mut fif = [b' '; 20];
        for (slot, c) in fif.iter_mut().zip(self.identification.bytes().rev()) {
            *slot = c;
        }
        self.sender.send(&[Message::new(Frame::Tsi, true).with_fif(&fif)]);
        self.phase = Phase::Answering;
    }

    fn hang_up(&mut self) {
        self.sender.send(&[Message::new(Frame::Dcn, true)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames;

    const FS: f64 = 16_000.0;
    /// A real machine's capability frame, off a recording of a public fax
    /// number answering.
    const DIS: [u8; 4] = [0x00, 0x6e, 0xf8, 0x00];
    const CSI: &[u8; 20] = b"       909 863  0031";

    /// Run a call, feeding it whatever the far end is made to say.
    fn run(far: &[Message], seconds: f64) -> Call {
        let mut call = Call::new(FS, "61400000000");
        let mut tx = frames::Sender::new();
        tx.send(far);
        for _ in 0..(seconds * FS) as usize {
            // The far end speaks at 300 bit/s; one bit every so many samples.
            let bit = if (call.seconds() * 300.0).fract() < 300.0 / FS {
                tx.next_bit()
            } else {
                None
            };
            call.advance(bit, true);
            // And drain whatever we are sending, as a line would.
            while call.next_bit().is_some() {}
            if call.phase() == Phase::Done || call.phase() == Phase::Failed {
                break;
            }
        }
        call
    }

    #[test]
    fn the_calling_tone_is_on_for_half_a_second_in_every_three_and_a_half() {
        let mut call = Call::new(FS, "1");
        let mut on = 0usize;
        let total = (FS * (crate::CNG_ON + crate::CNG_OFF)) as usize;
        for _ in 0..total {
            if call.wants_calling_tone() {
                on += 1;
            }
            call.advance(None, true);
        }
        let fraction = on as f64 / total as f64;
        let want = crate::CNG_ON / (crate::CNG_ON + crate::CNG_OFF);
        assert!(
            (fraction - want).abs() < 0.02,
            "the tone was on {fraction:.3} of the time, wanted {want:.3}"
        );
    }

    #[test]
    fn a_machine_that_says_what_it_is_gets_heard() {
        let call = run(
            &[
                Message::new(Frame::Csi, false).and_more().with_fif(CSI),
                Message::new(Frame::Dis, false).with_fif(&DIS),
            ],
            20.0,
        );
        assert_eq!(call.identity, "1300  368 909");
        let caps = call.capabilities.clone().expect("it said what it can do");
        assert_eq!(
            caps.modulations,
            vec![
                t30::Modulation::V27ter,
                t30::Modulation::V29,
                t30::Modulation::V17
            ]
        );
        assert!(caps.receives);
        assert_eq!(
            call.phase(),
            Phase::Done,
            "it should identify itself and hang up, not sit there"
        );
    }

    #[test]
    fn nothing_at_all_gives_up_after_t1_and_not_before() {
        let call = run(&[], T1_SECONDS - 2.0);
        assert_eq!(call.phase(), Phase::Calling, "gave up early");
        let call = run(&[], T1_SECONDS + 2.0);
        assert_eq!(call.phase(), Phase::Failed);
    }

    #[test]
    fn a_far_end_that_hangs_up_ends_the_call() {
        let call = run(&[Message::new(Frame::Dcn, false)], 20.0);
        assert_eq!(call.phase(), Phase::Done);
        assert!(call.capabilities.is_none());
    }

    #[test]
    fn our_identification_goes_out_backwards_as_the_recommendation_asks() {
        let mut call = Call::new(FS, "61400000000");
        call.capabilities = Some(t30::capabilities(&DIS));
        call.phase = Phase::Heard;
        call.advance(None, true);
        let mut bits = Vec::new();
        while let Some(b) = call.next_bit() {
            bits.push(b);
        }
        let mut reader = Reader::new();
        let sent: Vec<Message> = bits.iter().filter_map(|b| reader.feed(*b)).collect();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].frame, Frame::Tsi);
        assert!(sent[0].from_caller, "we are the end that dialled");
        assert_eq!(t30::identification(&sent[0].fif), "61400000000");
    }
}
