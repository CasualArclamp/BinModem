//! The `+++` escape sequence.
//!
//! **This is not V.250.** The Recommendation defines no escape sequence and no
//! S2 or S12; the mechanism below is Hayes practice, later written down in
//! TIA-602. It is implemented because Windows Dial-Up Networking and every
//! terminal program depend on it, but it is an extension, not conformance.
//!
//! The sequence is three copies of the escape character (S2) with:
//!
//! - at least S12 of idle line before the first,
//! - no more than S12 between consecutive characters,
//! - at least S12 of idle line after the third, during which nothing else
//!   may arrive.
//!
//! The guard times are what stop a file transfer that happens to contain `+++`
//! from dropping the modem into command state.

use crate::registers::Registers;

/// Escape character register (S2) and guard time register (S12), both Hayes
/// extensions. S2 defaults to `+` and S12 to 50 fiftieths of a second, i.e. one
/// second.
pub const S2_ESCAPE_CHAR: u8 = 2;
pub const S12_GUARD_TIME: u8 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for the pre-sequence idle period to elapse.
    NeedGuard,
    /// Guard time satisfied; ready to count escape characters.
    Armed,
    /// Counting escape characters.
    Counting(u8),
    /// Three seen; waiting out the trailing guard time.
    TrailingGuard,
}

/// Detects the escape sequence in the transmit data stream.
///
/// Time is supplied by the caller in milliseconds so the detector stays
/// testable and independent of any clock.
#[derive(Debug, Clone)]
pub struct EscapeDetector {
    phase: Phase,
    idle_ms: u32,
}

impl Default for EscapeDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl EscapeDetector {
    pub fn new() -> Self {
        Self { phase: Phase::NeedGuard, idle_ms: 0 }
    }

    /// Guard time in milliseconds, from S12 in fiftieths of a second.
    fn guard_ms(regs: &Registers) -> u32 {
        u32::from(regs.raw(S12_GUARD_TIME)) * 20
    }

    /// Advance the idle timer by `dt_ms` during which no data was sent.
    ///
    /// Returns true when the trailing guard time completes and the modem should
    /// enter online command state.
    pub fn idle(&mut self, dt_ms: u32, regs: &Registers) -> bool {
        let guard = Self::guard_ms(regs);
        self.idle_ms = self.idle_ms.saturating_add(dt_ms);

        match self.phase {
            Phase::NeedGuard => {
                if self.idle_ms >= guard {
                    self.phase = Phase::Armed;
                }
                false
            }
            // An over-long gap between escape characters voids the attempt.
            Phase::Counting(_) if self.idle_ms > guard => {
                self.phase = Phase::Armed;
                false
            }
            Phase::TrailingGuard if self.idle_ms >= guard => {
                self.phase = Phase::NeedGuard;
                self.idle_ms = 0;
                true
            }
            _ => false,
        }
    }

    /// Feed one byte of outbound data. Resets the timers, since any data at all
    /// breaks the idle requirement.
    pub fn data(&mut self, byte: u8, regs: &Registers) {
        let escape = regs.raw(S2_ESCAPE_CHAR);
        // S2 above 127 disables the escape sequence entirely, per Hayes practice.
        let enabled = escape < 128;

        self.phase = match (self.phase, enabled && byte == escape) {
            (Phase::Armed, true) => Phase::Counting(1),
            (Phase::Counting(2), true) => Phase::TrailingGuard,
            (Phase::Counting(n), true) => Phase::Counting(n + 1),
            // Any other character abandons the attempt and restarts the
            // leading guard requirement.
            _ => Phase::NeedGuard,
        };
        self.idle_ms = 0;
    }

    /// Abandon any partial sequence, on carrier loss or a return to data state.
    pub fn reset(&mut self) {
        self.phase = Phase::NeedGuard;
        self.idle_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regs() -> Registers {
        Registers::default()
    }

    /// S12 defaults to 50 fiftieths, so the guard time is one second.
    const GUARD: u32 = 1000;

    fn escaped(seq: &[u8], gap_ms: u32, lead_ms: u32, trail_ms: u32) -> bool {
        let r = regs();
        let mut d = EscapeDetector::new();
        d.idle(lead_ms, &r);
        for (i, &b) in seq.iter().enumerate() {
            if i > 0 {
                d.idle(gap_ms, &r);
            }
            d.data(b, &r);
        }
        d.idle(trail_ms, &r)
    }

    #[test]
    fn recognises_a_well_formed_sequence() {
        assert!(escaped(b"+++", 50, GUARD, GUARD));
    }

    #[test]
    fn rejects_a_sequence_without_leading_guard_time() {
        // Data was flowing right up to the "+++", so it is file content.
        assert!(!escaped(b"+++", 50, 0, GUARD));
    }

    #[test]
    fn rejects_a_sequence_without_trailing_guard_time() {
        assert!(!escaped(b"+++", 50, GUARD, GUARD / 4));
    }

    #[test]
    fn rejects_too_few_escape_characters() {
        assert!(!escaped(b"++", 50, GUARD, GUARD));
    }

    #[test]
    fn a_long_gap_between_characters_voids_the_attempt() {
        assert!(!escaped(b"+++", GUARD * 2, GUARD, GUARD));
    }

    #[test]
    fn data_interleaved_with_the_escape_characters_voids_it() {
        assert!(!escaped(b"++x+", 50, GUARD, GUARD));
    }

    #[test]
    fn plus_signs_inside_a_transfer_do_not_escape() {
        // The case the guard times exist to defend against.
        let r = regs();
        let mut d = EscapeDetector::new();
        for b in b"data+++more" {
            d.data(*b, &r);
        }
        assert!(!d.idle(GUARD * 5, &r));
    }

    #[test]
    fn s2_selects_the_escape_character() {
        let mut r = regs();
        r.set(2, b'X' as u32).unwrap();
        let mut d = EscapeDetector::new();
        d.idle(GUARD, &r);
        for _ in 0..3 {
            d.data(b'X', &r);
            d.idle(50, &r);
        }
        assert!(d.idle(GUARD, &r), "XXX should escape when S2 is 'X'");

        let mut d = EscapeDetector::new();
        d.idle(GUARD, &r);
        for _ in 0..3 {
            d.data(b'+', &r);
            d.idle(50, &r);
        }
        assert!(!d.idle(GUARD, &r), "+++ should not escape when S2 is 'X'");
    }

    #[test]
    fn s2_above_127_disables_escaping() {
        let mut r = regs();
        r.set(2, 255).unwrap();
        let mut d = EscapeDetector::new();
        d.idle(GUARD, &r);
        for _ in 0..3 {
            d.data(255, &r);
            d.idle(50, &r);
        }
        assert!(!d.idle(GUARD, &r));
    }

    #[test]
    fn s12_scales_the_guard_time() {
        let mut r = regs();
        r.set(12, 25).unwrap(); // 25/50 s = 500 ms
        let mut d = EscapeDetector::new();
        d.idle(500, &r);
        for _ in 0..3 {
            d.data(b'+', &r);
            d.idle(20, &r);
        }
        assert!(d.idle(500, &r), "500 ms should suffice when S12 is 25");
    }
}
