//! Bell 103 — 300 bps full-duplex FSK.
//!
//! Full duplex is achieved by frequency division, so the two directions occupy
//! separate bands and no echo canceller is required:
//!
//! | direction | space | mark |
//! |---|---|---|
//! | originating | 1070 Hz | 1270 Hz |
//! | answering | 2025 Hz | 2225 Hz |
//!
//! Bell 103 is the North American 300 bps standard; ITU-T V.21 is the
//! equivalent elsewhere and differs only in tone assignment (980/1180 and
//! 1650/1850), so the same receiver serves both.

use dsp::FskDetector;

use crate::framing::AsyncFramer;

/// Which end of the call this modem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Originate,
    Answer,
}

impl Role {
    /// The band this role transmits in, as `(space, mark)`.
    pub fn transmit_tones(self) -> (f64, f64) {
        match self {
            Role::Originate => (1070.0, 1270.0),
            Role::Answer => (2025.0, 2225.0),
        }
    }

    /// The band this role listens to: whatever the far end transmits.
    pub fn receive_tones(self) -> (f64, f64) {
        match self {
            Role::Originate => Role::Answer.transmit_tones(),
            Role::Answer => Role::Originate.transmit_tones(),
        }
    }
}

pub const BAUD: f64 = 300.0;

/// A streaming Bell 103 receiver: line samples in, characters out.
#[derive(Debug)]
pub struct Bell103Rx {
    detector: FskDetector,
    framer: AsyncFramer,
    last_level: f64,
    symbol: Option<f64>,
}

impl Bell103Rx {
    /// Build a receiver listening to the band the far end of `role` transmits in.
    pub fn new(role: Role, fs: f64) -> Self {
        let (space, mark) = role.receive_tones();
        Self::with_tones(space, mark, fs)
    }

    /// Build a receiver for an explicit tone pair, for V.21 or for tapping a
    /// specific direction out of a 2-wire capture.
    pub fn with_tones(space: f64, mark: f64, fs: f64) -> Self {
        Self {
            detector: FskDetector::new(space, mark, BAUD, fs),
            framer: AsyncFramer::new(BAUD, fs, 8),
            last_level: 0.0,
            symbol: None,
        }
    }

    /// Feed one line sample; yields a character when a frame completes.
    #[inline]
    pub fn feed(&mut self, sample: f64) -> Option<u8> {
        let level = self.detector.feed(sample);
        self.last_level = level;
        let carrier = self.detector.carrier();
        let out = self.framer.feed(level, carrier);
        self.symbol = self.framer.take_sampled();
        out
    }

    /// Discriminator level of the bit sampled on the last `feed`, if any.
    ///
    /// One value per recovered bit, taken at the bit centre. This is what the
    /// symbol scope plots: distance from zero is the slicer's decision margin.
    pub fn take_symbol(&mut self) -> Option<f64> {
        self.symbol.take()
    }

    pub fn carrier(&self) -> bool {
        self.detector.carrier()
    }

    /// Most recent discriminator output: `+1` is a mark, `-1` a space.
    ///
    /// This is what an eye diagram is drawn from, and the only meaningful scope
    /// for FSK — there is no constellation to plot.
    pub fn level(&self) -> f64 {
        self.last_level
    }

    /// Received signal envelope in this band, for a level meter.
    pub fn amplitude(&self) -> f64 {
        self.detector.level()
    }

    pub fn framing_errors(&self) -> u64 {
        self.framer.framing_errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_listen_to_the_opposite_band() {
        assert_eq!(Role::Originate.receive_tones(), (2025.0, 2225.0));
        assert_eq!(Role::Answer.receive_tones(), (1070.0, 1270.0));
    }
}
