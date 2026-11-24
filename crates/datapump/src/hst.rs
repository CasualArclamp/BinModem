//! USRobotics HST: an asymmetric pair of channels on one line.
//!
//! HST was never published. What is here comes from reading the firmware of
//! a Courier V.Everything (flash build 06/09/95: the 80188 supervisor and the
//! TMS320C50 DSP code), and the notes of that work give every address. In
//! outline:
//!
//! - **Forward channel**, the fast one: 2400 symbols a second on an 1800 Hz
//!   carrier, sampled by the Courier at 7200 Hz. Its mapper reads the same
//!   constellation and trellis tables, with the same index arithmetic, as the
//!   firmware's V.32 transmitter: 4800 uncoded, 7200 to 14 400 trellis coded
//!   as V.32bis does, and 16 800 on a 256-point set. Its scrambler is
//!   V.32's calling-modem polynomial, 1 + x^-18 + x^-23. The Courier's pulse
//!   is much sharper than V.32's: flat to 1100 Hz off the carrier, 3 dB down
//!   at 1200 and 36 dB down at 1400, which leaves the band below about
//!   450 Hz clear for the back channel.
//! - **Back channel**, the slow one: differential eight-phase, three bits a
//!   symbol, at 150 baud on 375 Hz (450 bit/s) or 100 baud on 350 Hz
//!   (300 bit/s). The phase step for each scrambled tribit is the firmware's
//!   table; its scrambler is 1 + x^-14 + x^-17.
//! - Whichever end has more to send holds the forward channel, and the two
//!   swap with attention frames in the error-control layer above this one.
//!
//! This module is the modulation layer only, and it is built for two of
//! these modems talking to each other. The forward channel here runs on the
//! V.17 engine, which is that same V.32bis modulation sent one way with a
//! fixed training; its start-up (V.17's long train) and pulse (V.17's
//! 25% roll-off, cut back by a guard filter) are not the Courier's yet, so a
//! real Courier will not train on it. The back channel is written from the
//! firmware's description, but its pulse shape and preamble are this
//! module's own.

mod back;
pub mod call;
pub mod session;

pub use back::{BackRate, BackRx, BackTx};
pub use session::{Config, Holding, Session, Side};

/// Symbols of preamble the back-channel transmitter sends.
pub fn back_preamble() -> u32 {
    back::PREAMBLE_SYMBOLS
}

use dsp::{Cascade, butter_highpass};

use crate::v17;

/// Forward-channel symbol rate.
pub const FORWARD_BAUD: f64 = 2400.0;
/// Forward-channel carrier, in hertz.
pub const FORWARD_CARRIER: f64 = 1800.0;

/// Forward rates this implementation carries.
///
/// HST's list is 4800 to 16 800 in steps of 2400. The V.17 engine underneath
/// covers 7200 to 14 400, which includes the 9600 of a Courier 9600 HST; 4800
/// (uncoded) and 16 800 (a 256-point set that only V.32terbo published) are
/// not here yet.
pub use v17::Rate;

/// The rate code the Courier's own software uses for a forward rate: the
/// number of data bits a symbol carries, 2 for 4800 to 7 for 16 800. Its
/// channel-control messages carry this number.
pub fn rate_code(rate: Rate) -> u8 {
    rate.bits() as u8
}

/// The forward rate for a Courier rate code, where this implementation has it.
pub fn rate_from_code(code: u8) -> Option<Rate> {
    Rate::of(u32::from(code) * 2400)
}

/// Corner of the guard filter that keeps the forward channel out of the back
/// channel's band, in both directions: on the forward transmitter's output,
/// so the far back-channel receiver is not swamped by our echo, and in front
/// of the forward receiver, so our own back channel's echo does not reach it.
pub const FORWARD_GUARD_HZ: f64 = 560.0;
/// Order of the guard filter.
const FORWARD_GUARD_ORDER: usize = 10;

fn forward_guard(fs: f64) -> Cascade {
    butter_highpass(FORWARD_GUARD_ORDER, FORWARD_GUARD_HZ, fs)
}

/// Forward-channel transmitter: the fast direction.
#[derive(Debug)]
pub struct ForwardTx {
    tx: v17::Transmitter,
    guard: Cascade,
}

impl ForwardTx {
    pub fn new(fs: f64) -> Self {
        Self { tx: v17::Transmitter::new(fs), guard: forward_guard(fs) }
    }

    /// Raise the carrier and train the far receiver from nothing at `rate`.
    pub fn start(&mut self, rate: Rate) {
        self.guard.reset();
        self.tx.start(rate, v17::Training::Long);
    }

    /// Finish what is queued and drop the carrier.
    pub fn stop(&mut self) {
        self.tx.stop();
    }

    /// Drop the carrier now.
    pub fn abort(&mut self) {
        self.tx.abort();
    }

    pub fn rate(&self) -> Rate {
        self.tx.rate()
    }

    /// Whether the training is over and queued bits are going out.
    pub fn trained(&self) -> bool {
        self.tx.trained()
    }

    pub fn is_transmitting(&self) -> bool {
        self.tx.is_transmitting()
    }

    /// Queue bits, first to go first. With nothing queued it sends ones.
    pub fn push_bits(&mut self, bits: &[bool]) {
        self.tx.push_bits(bits);
    }

    pub fn pending_bits(&self) -> usize {
        self.tx.pending_bits()
    }

    pub fn next_sample(&mut self) -> f64 {
        let s = self.tx.next_sample();
        self.guard.process(s)
    }
}

/// Forward-channel receiver.
#[derive(Debug)]
pub struct ForwardRx {
    rx: v17::Receiver,
    guard: Cascade,
}

impl ForwardRx {
    pub fn new(fs: f64) -> Self {
        Self { rx: v17::Receiver::new(fs), guard: forward_guard(fs) }
    }

    /// The rate to decode once trained, which both ends agree in advance,
    /// and listen for a new training at it.
    pub fn set_rate(&mut self, rate: Rate) {
        self.rx.set_rate(rate);
        self.restart();
    }

    pub fn rate(&self) -> Rate {
        self.rx.rate()
    }

    /// Forget the line and listen for a new training.
    pub fn restart(&mut self) {
        self.guard.reset();
        self.rx.restart();
    }

    pub fn feed(&mut self, sample: f64) {
        let s = self.guard.process(sample);
        self.rx.feed(s);
    }

    /// Bits decoded since the last call, first received first.
    pub fn take_bits(&mut self) -> Vec<bool> {
        self.rx.take_bits()
    }

    /// Whether the receiver has finished training and is passing data.
    pub fn in_data(&self) -> bool {
        self.rx.stage() == v17::Stage::Data
    }

    pub fn carrier(&self) -> bool {
        self.rx.carrier()
    }

    pub fn snr_db(&self) -> f64 {
        self.rx.snr_db()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Courier frames its error control as V.42 does: the check below is
    /// what its CRC routine (8FBB:1874, preset 0xFFFF, checked against the
    /// residue 0xF0B8) gives, so `ec::hdlc` can frame HST without change.
    #[test]
    fn the_frame_check_is_the_couriers() {
        let mut crc = ec::hdlc::Crc16::new();
        crc.update_all(b"123456789");
        assert_eq!(crc.to_bytes(), [0x6E, 0x90]);
        let framed = ec::hdlc::append_fcs(b"123456789", ec::hdlc::Fcs::Bits16);
        let mut check = ec::hdlc::Crc16::new();
        check.update_all(&framed);
        assert_eq!(check.residue(), 0xF0B8);
    }

    #[test]
    fn rate_codes_are_bits_per_symbol() {
        assert_eq!(rate_code(Rate::R9600), 4);
        assert_eq!(rate_code(Rate::R14400), 6);
        assert_eq!(rate_from_code(3), Some(Rate::R7200));
        assert_eq!(rate_from_code(5), Some(Rate::R12000));
        assert_eq!(rate_from_code(2), None);
        assert_eq!(rate_from_code(7), None);
    }
}
