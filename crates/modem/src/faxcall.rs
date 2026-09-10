//! A fax call on the line: the tones, V.21, and T.30 above them.
//!
//! The join between the two halves. [`fax::call`] knows the procedure and
//! nothing about signals; [`datapump::v21`] knows the signals and nothing
//! about the procedure. This puts one on top of the other and gives the
//! result a sample at a time, which is the only thing a line understands.

use datapump::v21;
use fax::call::{Call, Phase};

/// A fax call, from the end that dialled.
#[derive(Debug)]
pub struct FaxCall {
    call: Call,
    tx: v21::Sender,
    rx: v21::Receiver,
    cng: v21::Tone,
    /// Whether the V.21 carrier is up, which has to be turned on before the
    /// preamble and left on until the closing flag has gone.
    sending: bool,
}

impl FaxCall {
    pub fn new(fs: f64, identification: &str) -> Self {
        Self {
            call: Call::new(fs, identification),
            tx: v21::Sender::new(fs),
            rx: v21::Receiver::new(fs),
            cng: v21::Tone::new(v21::CNG, fs),
            sending: false,
        }
    }

    pub fn phase(&self) -> Phase {
        self.call.phase()
    }

    pub fn seconds(&self) -> f64 {
        self.call.seconds()
    }

    pub fn identity(&self) -> &str {
        &self.call.identity
    }

    pub fn capabilities(&self) -> Option<&fax::t30::Capabilities> {
        self.call.capabilities.as_ref()
    }

    /// The capability field as it arrived.
    pub fn capability_field(&self) -> Option<&[u8]> {
        self.call.capability_field.as_deref()
    }

    pub fn take_heard(&mut self) -> Vec<fax::frames::Message> {
        self.call.take_heard()
    }

    /// Whether the far end's control channel is on the line.
    pub fn carrier(&self) -> bool {
        self.rx.carrier()
    }

    /// One sample in, one sample out.
    ///
    /// The receiver is fed whatever arrives even while this end is
    /// transmitting. On a two-wire line that means it hears itself, which is
    /// harmless here: a fax is half duplex, so anything it hears while
    /// sending is its own echo and the frames in it are the frames it just
    /// sent. They are addressed the same way and would be read as the far
    /// end's, so the procedure ignores whatever arrives while it is talking.
    pub fn step(&mut self, line: f64) -> f64 {
        let bit = if self.sending { None } else { self.rx.feed(line) };
        // The line is idle when nothing is queued in the modulator either,
        // not merely when the procedure has handed everything over.
        let idle = !self.sending && self.tx.pending_bits() == 0;
        self.call.advance(bit, idle);

        // Keep the transmitter fed while there is a burst to send, and take
        // the carrier down once the last flag has gone out.
        let wanted = self.call.wants_carrier();
        if wanted && !self.sending {
            self.sending = true;
            self.tx.set_transmitting(true);
        }
        if self.sending {
            while self.tx.pending_bits() < 16 {
                match self.call.next_bit() {
                    Some(bit) => self.tx.push_bits(&[bit]),
                    None => break,
                }
            }
            if !wanted && self.tx.pending_bits() == 0 {
                self.sending = false;
                self.tx.set_transmitting(false);
            }
            return self.tx.next_sample();
        }

        if self.call.wants_calling_tone() {
            return self.cng.next_sample();
        }
        // The tone keeps running while it is silent, so its phase is
        // continuous across the gaps rather than clicking at every burst.
        self.cng.next_sample();
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fax::frames::{Message, Reader};
    use fax::t30::Frame;

    const FS: f64 = 16_000.0;

    #[test]
    fn the_calling_tone_goes_on_the_line() {
        let mut call = FaxCall::new(FS, "61400000000");
        let mut loudest = 0.0f64;
        for _ in 0..(FS * 0.3) as usize {
            loudest = loudest.max(call.step(0.0).abs());
        }
        assert!(loudest > 0.1, "nothing went out: {loudest}");
    }

    #[test]
    fn a_machine_answering_is_heard_and_answered() {
        // The far end is a recording of a real one: its identification and
        // its capabilities, sent on V.21 as it would send them.
        let mut far_tx = fax::frames::Sender::new();
        far_tx.send(&[
            Message::new(Frame::Csi, false)
                .and_more()
                .with_fif(b"       909 863  0031"),
            Message::new(Frame::Dis, false).with_fif(&[0x00, 0x6e, 0xf8, 0x00]),
        ]);
        let mut far = v21::Sender::new(FS);
        far.set_transmitting(true);

        let mut call = FaxCall::new(FS, "61400000000");
        // Read back what this end says, so the reply can be checked.
        let mut ours = v21::Receiver::new(FS);
        let mut reader = Reader::new();
        let mut said: Vec<Message> = Vec::new();

        for _ in 0..(FS * 12.0) as usize {
            while far.pending_bits() < 16 {
                match far_tx.next_bit() {
                    Some(b) => far.push_bits(&[b]),
                    None => break,
                }
            }
            let from_far = far.next_sample();
            let from_us = call.step(from_far);
            if let Some(bit) = ours.feed(from_us)
                && let Some(m) = reader.feed(bit)
            {
                said.push(m);
            }
            if call.phase() == Phase::Done {
                break;
            }
        }

        assert_eq!(call.identity(), "1300  368 909");
        let caps = call.capabilities().expect("it said what it can do");
        assert_eq!(caps.modulations.len(), 3, "V.27ter, V.29 and V.17");
        assert_eq!(call.phase(), Phase::Done);

        let names: Vec<Frame> = said.iter().map(|m| m.frame).collect();
        assert!(
            names.contains(&Frame::Tsi),
            "this end never identified itself: {names:?}"
        );
        assert!(
            names.contains(&Frame::Dcn),
            "this end never hung up politely: {names:?}"
        );
    }
}
