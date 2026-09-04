//! A complete V.42 endpoint: compression over error control over framing.
//!
//! The three layers are separately testable and separately useless. Data from
//! the terminal is compressed by V.42bis, carried in the information field of
//! a LAPM frame, wrapped in HDLC with a check sequence, and handed to the data
//! pump a bit at a time; and the reverse coming back. What the layers need
//! from each other is narrow but fiddly, and every place that wanted a working
//! link was assembling it by hand.
//!
//! The line side is a bit at a time and never runs dry. A synchronous
//! connection always carries something, and when there is nothing to say that
//! something is the flag: it keeps the far end's framing synchronised, and its
//! absence is how a modem notices the connection has gone.

use crate::frame::{Address, Frame, Role};
use crate::hdlc::{Decoder, Encoder, Fcs};
use crate::lapm::{Event, Lapm, Params, State};
use crate::v42bis;

/// The data link both ends use for user data (V.42 8.1.2).
const DLCI_DATA: u8 = 0;

/// One end of a V.42 connection.
#[derive(Debug)]
pub struct Stack {
    role: Role,
    lapm: Lapm,
    encoder: Encoder,
    decoder: Decoder,
    /// Compression, when it has been agreed. V.42bis is optional and a
    /// connection without it is a perfectly ordinary V.42 connection.
    compression: Option<Compressor>,
    /// Data ready for the terminal.
    delivered: Vec<u8>,
    /// Frames that arrived but could not be read, which is the measure of how
    /// the line is behaving.
    damaged: u64,
}

#[derive(Debug)]
struct Compressor {
    encoder: v42bis::Encoder,
    decoder: v42bis::Decoder,
}

impl Stack {
    pub fn new(role: Role, params: Params) -> Self {
        Self {
            role,
            lapm: Lapm::new(role, DLCI_DATA, params),
            encoder: Encoder::new(Fcs::Bits16),
            decoder: Decoder::new(Fcs::Bits16),
            compression: None,
            delivered: Vec::new(),
            damaged: 0,
        }
    }

    /// Turn on V.42bis, which both ends must have agreed to by XID first.
    ///
    /// Turning it on at one end only would not fail cleanly: the far end would
    /// decompress data that was never compressed and deliver nonsense.
    pub fn with_compression(mut self, params: v42bis::Params) -> Self {
        self.compression = Some(Compressor {
            encoder: v42bis::Encoder::new(params),
            decoder: v42bis::Decoder::new(params),
        });
        self
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn state(&self) -> State {
        self.lapm.state()
    }

    pub fn is_connected(&self) -> bool {
        self.lapm.is_connected()
    }

    /// Frames that arrived damaged and were dropped.
    pub fn damaged_frames(&self) -> u64 {
        self.damaged
    }

    /// Ask for the link to be established.
    pub fn connect(&mut self) {
        self.lapm.connect();
    }

    /// Ask for it to be released.
    pub fn disconnect(&mut self) {
        self.lapm.disconnect();
    }

    /// Time passing, which is what drives retransmission.
    pub fn tick(&mut self, dt_ms: u32) {
        self.lapm.tick(dt_ms);
        self.drain();
    }

    /// Queue data from the terminal.
    pub fn send(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        match &mut self.compression {
            Some(c) => {
                let mut out = Vec::new();
                c.encoder.encode(data, &mut out);
                // Flush, because the terminal has no idea it is being
                // compressed and will sit waiting for an echo of what it
                // typed. A dictionary coder that holds the last few characters
                // back for a better match would look exactly like a hung line.
                c.encoder.flush(&mut out);
                self.lapm.send_data(&out);
            }
            None => self.lapm.send_data(data),
        }
    }

    /// Data that has arrived, decompressed and in order.
    pub fn take_received(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.delivered)
    }

    /// One bit for the line.
    ///
    /// Never nothing: a synchronous link always carries something, and when
    /// there is nothing to say it carries flags.
    pub fn next_bit(&mut self) -> bool {
        if self.encoder.is_empty() {
            let mut queued = false;
            while let Some((frame, kind)) = self.lapm.poll_transmit() {
                let body = frame.encode(DLCI_DATA, self.role, kind);
                self.encoder.frame(&body);
                queued = true;
            }
            if !queued {
                self.encoder.idle(1);
            }
        }
        self.encoder.next_bit().unwrap_or(true)
    }

    /// One bit from the line.
    pub fn feed_bit(&mut self, bit: bool) {
        let Some(result) = self.decoder.feed(bit) else {
            return;
        };
        let Ok(body) = result else {
            // A frame that did not survive the line is dropped and left to the
            // retransmission machinery, which is what it is for. Counting them
            // is worth doing: it is the difference between a link that is
            // working and one that is only apparently working.
            self.damaged += 1;
            return;
        };
        let Ok((address, frame)) = Frame::decode(&body, self.role) else {
            self.damaged += 1;
            return;
        };
        self.dispatch(address, frame);
    }

    fn dispatch(&mut self, address: Address, frame: Frame) {
        self.lapm.receive(frame, address.kind);
        self.drain();
    }

    fn drain(&mut self) {
        let mut arrived: Vec<u8> = Vec::new();
        while let Some(event) = self.lapm.poll_event() {
            if let Event::Data(d) = event {
                arrived.extend_from_slice(&d);
            }
        }
        if arrived.is_empty() {
            return;
        }
        match &mut self.compression {
            Some(c) => {
                if c.decoder.decode(&arrived, &mut self.delivered).is_err() {
                    // A compressed stream that will not decode cannot be
                    // recovered from by asking again: the dictionary at each
                    // end is built from everything that came before, so once
                    // they disagree they stay disagreed. V.42bis 6.4 has the
                    // receiver ask for the link to be reset.
                    self.damaged += 1;
                    self.lapm.disconnect();
                }
            }
            None => self.delivered.extend_from_slice(&arrived),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run two ends against each other until neither has anything to say.
    ///
    /// `channel` is given each bit and returns what arrives, which is where a
    /// test puts errors.
    fn settle(a: &mut Stack, b: &mut Stack, bits: usize, mut channel: impl FnMut(usize, bool) -> bool) {
        for i in 0..bits {
            let to_b = a.next_bit();
            let to_a = b.next_bit();
            b.feed_bit(channel(i, to_b));
            a.feed_bit(channel(i, to_a));
            if i % 160 == 0 {
                // A bit at 9600 is about a tenth of a millisecond, so this is
                // roughly real time for the retransmission timers.
                a.tick(16);
                b.tick(16);
            }
        }
    }

    fn pair() -> (Stack, Stack) {
        (
            Stack::new(Role::Originator, Params::default()),
            Stack::new(Role::Answerer, Params::default()),
        )
    }

    #[test]
    fn a_link_establishes_and_carries_data() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        assert!(a.is_connected(), "the originator is {:?}", a.state());
        assert!(b.is_connected(), "the answerer is {:?}", b.state());

        a.send(b"login: cactus\r\n");
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        assert_eq!(b.take_received(), b"login: cactus\r\n");
    }

    #[test]
    fn an_idle_link_carries_flags_rather_than_nothing() {
        // A synchronous connection has to keep something on the line: the far
        // end's framing stays synchronised on it, and its absence is how a
        // modem tells that the connection has gone.
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        let idle: Vec<bool> = (0..80).map(|_| a.next_bit()).collect();
        // Where in a flag the stream was caught is arbitrary, so look for the
        // alignment at which it reads as flags rather than assuming one.
        let aligned = (0..8).any(|offset| {
            idle[offset..offset + 64]
                .chunks(8)
                .all(|c| c.iter().rev().fold(0u8, |acc, &b| (acc << 1) | u8::from(b)) == 0x7e)
        });
        assert!(
            aligned,
            "an idle link carried {:?} rather than flags",
            idle.iter().map(|&b| u8::from(b)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn data_crosses_in_both_directions_at_once() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        a.send(b"what the caller typed");
        b.send(b"what the host answered");
        settle(&mut a, &mut b, 40_000, |_, bit| bit);
        assert_eq!(b.take_received(), b"what the caller typed");
        assert_eq!(a.take_received(), b"what the host answered");
    }

    #[test]
    fn a_line_that_damages_frames_still_delivers() {
        // The point of error control. Every so often a bit is flipped, which
        // fails a check sequence and loses a whole frame; what arrives at the
        // far end must still be exactly what was sent.
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);

        let payload: Vec<u8> = (0..400).map(|i| (i % 251) as u8).collect();
        a.send(&payload);
        settle(&mut a, &mut b, 400_000, |i, bit| {
            if i % 4099 == 0 { !bit } else { bit }
        });
        assert_eq!(b.take_received(), payload);
        assert!(
            a.damaged_frames() + b.damaged_frames() > 0,
            "the channel did not actually damage anything, so nothing was tested"
        );
    }

    #[test]
    fn compression_is_carried_through_the_same_interface() {
        let params = v42bis::Params::default();
        let mut a = Stack::new(Role::Originator, Params::default()).with_compression(params);
        let mut b = Stack::new(Role::Answerer, Params::default()).with_compression(params);
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);

        // Something with the repetition a dictionary coder exists for.
        let payload = b"the same words over and over, the same words over and over, \
                        the same words over and over, the same words over and over"
            .to_vec();
        a.send(&payload);
        settle(&mut a, &mut b, 80_000, |_, bit| bit);
        assert_eq!(b.take_received(), payload);
    }

    #[test]
    fn a_release_is_seen_at_both_ends() {
        let (mut a, mut b) = pair();
        a.connect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        assert!(a.is_connected() && b.is_connected());
        a.disconnect();
        settle(&mut a, &mut b, 20_000, |_, bit| bit);
        assert!(!a.is_connected(), "the originator is {:?}", a.state());
        assert!(!b.is_connected(), "the answerer is {:?}", b.state());
    }
}
