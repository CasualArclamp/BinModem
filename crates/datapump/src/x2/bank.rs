//! x2's downstream levels: which octets each of a frame's six positions may
//! carry at each rate and pad.
//!
//! A rate index n (1 to 15) is what the client's MP asks for, in word 0 bits
//! 2 to 5; the pad mode (0 none, 1 3 dB, 2 6 dB) is what it measured, in bits
//! 11 to 14. Each index carries B data bits a frame in mixed radix over the
//! six positions, plus MD = 5 signs, so the payload is (B + 5) x 8000 / 6
//! bit/s: 32000 at index 1, 56000 at 15 (U.S. Robotics prints a figure one
//! 1333 step higher, x2comref). Position 0 may have a list of its own;
//! positions 1 to 5 always share one.
//!
//! Sources: libv90modem's `docs/history/x2-facts.md` ("Mapping", "Banks",
//! "Pads"), which takes them from D-Modem's `docs/dsp/x2-details.md` and
//! courier-emu's x2 specification; the banks themselves are in `banks.rs`.

/// One position's levels, the largest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct List {
    /// The octets the client receives (positive mu-law).
    pub recv: &'static [u8],
    /// The octets sent so that they arrive as `recv` across the pad.
    pub sent: &'static [u8],
}

impl List {
    pub fn len(&self) -> usize {
        self.recv.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recv.is_empty()
    }
}

/// One rate's levels at one pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bank {
    /// Data bits a frame, besides the five signs.
    pub b: usize,
    /// Position 0's levels.
    pub first: List,
    /// Positions 1 to 5's.
    pub rest: List,
}

impl Bank {
    /// The bank for rate index `index` (1 to 15) behind pad mode `pad` (0 to
    /// 2), or `None` where there is none: a client that asks for one of those
    /// cannot be served.
    pub fn get(index: u8, pad: u8) -> Option<&'static Bank> {
        if !(1..=15).contains(&index) {
            return None;
        }
        super::banks::BANKS
            .get(usize::from(pad))
            .map(|row| &row[usize::from(index) - 1])
    }

    pub fn list(&self, position: usize) -> &List {
        if position == 0 { &self.first } else { &self.rest }
    }

    /// The payload rate in bit/s.
    pub fn rate(&self) -> u32 {
        (self.b as u32 + MD as u32) * 8000 / 6
    }

    /// Levels A: the sync frames', the records' and the start's first frame
    /// (x2-facts "Mapping": the I-modem's table at DSP `d238`, and D-Modem's).
    /// The same octets are sent whatever the pad, since the client measures
    /// the pad from them.
    pub const LEVELS_A: Bank = Bank {
        b: 19,
        first: LEVELS_A,
        rest: LEVELS_A,
    };
}

const LEVELS_A: List = List {
    recv: &[0xa5, 0xa7, 0xad, 0xaf, 0xb7, 0xbd, 0xc5, 0xcf, 0xe5],
    sent: &[0xa5, 0xa7, 0xad, 0xaf, 0xb7, 0xbd, 0xc5, 0xcf, 0xe5],
};

/// Signs a frame.
pub const MD: usize = 5;
/// Octets a frame.
pub const FRAME: usize = 6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_index_and_pad_has_a_bank_and_its_rate_climbs_in_steps() {
        for pad in 0..3 {
            let rates: Vec<u32> = (1..=15).map(|n| Bank::get(n, pad).unwrap().rate()).collect();
            // x2-facts "Banks": B = 19, 22, 25 for 1 to 3, then 22 + n.
            assert_eq!(rates[0], 32000);
            assert_eq!(rates[1], 36000);
            assert_eq!(rates[2], 40000);
            for n in 4..=15u32 {
                assert_eq!(rates[n as usize - 1], (22 + n + 5) * 8000 / 6, "index {n}");
            }
            assert_eq!(rates[14], 56000);
        }
        assert!(Bank::get(0, 0).is_none());
        assert!(Bank::get(16, 0).is_none());
        assert!(Bank::get(1, 3).is_none());
    }

    #[test]
    fn index_one_behind_the_pads_is_the_quads_table() {
        // x2-facts "Pads": the octets the Quad sends at index 1 behind 3 and
        // 6 dB, and what arrives.
        let b = Bank::get(1, 1).unwrap();
        assert_eq!(b.first.sent, &[0x9d, 0x9f, 0xa3, 0xa7, 0xac, 0xb2, 0xba, 0xc5, 0xdb]);
        assert_eq!(b.first.recv, &[0xa5, 0xa8, 0xab, 0xae, 0xb4, 0xba, 0xc0, 0xcc, 0xe0]);
        let b = Bank::get(1, 2).unwrap();
        assert_eq!(b.first.sent, &[0x95, 0x98, 0x9b, 0x9e, 0xa4, 0xaa, 0xb0, 0xbc, 0xd3]);
        assert_eq!(b.first.recv, Bank::get(1, 0).unwrap().first.recv);
    }
}
