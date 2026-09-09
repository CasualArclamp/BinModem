//! Sequence numbers, and the one thing that is easy to get wrong about them.
//!
//! RFC 9293 3.4: "the actual sequence number space is finite, though large.
//! This space ranges from 0 to 2^32 - 1. Since the space is finite, all
//! arithmetic dealing with sequence numbers must be performed modulo 2^32.
//! This unsigned arithmetic preserves the relationship of sequence numbers as
//! they cycle from 2^32 - 1 to 0 again. There are some subtleties to computer
//! modulo arithmetic, so great care should be taken in programming the
//! comparison of such values."
//!
//! The care it asks for is this: a sequence number is not a number that can be
//! compared with `<`. Two of them have no order at all in the abstract -- only
//! a distance, and only a direction once you assume the two are within half the
//! space of each other, which every real connection is by an enormous margin.
//! So the comparison is done on the *difference*, read as signed. That is the
//! whole of this file, and it is a separate file so nothing anywhere else is
//! tempted to write `<`.

use std::fmt;

/// One point in the sequence space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Seq(pub u32);

impl Seq {
    /// The distance from `self` forward to `other`, which is what a window or
    /// an unacknowledged span is measured in.
    ///
    /// Read as unsigned, so a number behind this one comes out as very nearly
    /// the whole space rather than as a negative. Callers that could be handed
    /// either want [`Seq::before`] first.
    pub fn distance_to(self, other: Seq) -> u32 {
        other.0.wrapping_sub(self.0)
    }

    /// Whether `self` comes before `other`: 9293's "<" over the sequence space.
    pub fn before(self, other: Seq) -> bool {
        (self.0.wrapping_sub(other.0) as i32) < 0
    }

    /// 9293's "=<".
    pub fn before_or_at(self, other: Seq) -> bool {
        !other.before(self)
    }

    pub fn after(self, other: Seq) -> bool {
        other.before(self)
    }

    pub fn after_or_at(self, other: Seq) -> bool {
        !self.before(other)
    }

    /// Whether `self` is inside the half-open span from `start` for `len`.
    ///
    /// The shape every window test in 3.4 takes: `start =< self < start + len`.
    /// A span of zero contains nothing, which is what makes the zero-window
    /// row of Table 5 a separate case rather than a special value here.
    pub fn within(self, start: Seq, len: u32) -> bool {
        start.distance_to(self) < len
    }
}

impl std::ops::Add<u32> for Seq {
    type Output = Seq;

    fn add(self, octets: u32) -> Seq {
        Seq(self.0.wrapping_add(octets))
    }
}

impl std::ops::AddAssign<u32> for Seq {
    fn add_assign(&mut self, octets: u32) {
        self.0 = self.0.wrapping_add(octets);
    }
}

impl std::ops::Sub<u32> for Seq {
    type Output = Seq;

    fn sub(self, octets: u32) -> Seq {
        Seq(self.0.wrapping_sub(octets))
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_numbers_compare_the_ordinary_way() {
        assert!(Seq(1).before(Seq(2)));
        assert!(!Seq(2).before(Seq(1)));
        assert!(Seq(2).after(Seq(1)));
        assert!(Seq(2).before_or_at(Seq(2)));
        assert!(Seq(2).after_or_at(Seq(2)));
        assert!(!Seq(2).before(Seq(2)));
    }

    /// The point of the whole file: the space wraps, and a number just past
    /// the end is after one just before it, not two billion before it.
    #[test]
    fn the_space_wraps_where_it_ends() {
        let last = Seq(u32::MAX);
        assert!(last.before(Seq(0)), "the end of the space is not before zero");
        assert!(last.before(Seq(5)));
        assert!(Seq(0).after(last));
        assert_eq!(last.distance_to(Seq(5)), 6);
        assert_eq!(last + 1, Seq(0));
        assert_eq!(Seq(0) - 1, last);
    }

    /// And the ordering holds all the way round, from any starting point.
    #[test]
    fn a_walk_round_the_whole_space_never_goes_backwards() {
        // A quarter of the space at a time, which visits every quadrant and
        // crosses the wrap twice.
        let step = 1u32 << 30;
        let mut at = Seq(0x8000_0000);
        for _ in 0..9 {
            let next = at + step;
            assert!(at.before(next), "{at} was not before {next}");
            assert!(next.after(at));
            assert_eq!(at.distance_to(next), step);
            at = next;
        }
    }

    /// 3.4's window test, which is the one every arriving segment goes
    /// through: RCV.NXT =< SEG.SEQ < RCV.NXT+RCV.WND.
    #[test]
    fn a_window_holds_what_is_inside_it_and_nothing_else() {
        let start = Seq(1000);
        assert!(start.within(start, 10), "the first octet is in the window");
        assert!(Seq(1009).within(start, 10));
        assert!(!Seq(1010).within(start, 10), "one past the end is not in it");
        assert!(!Seq(999).within(start, 10), "one before it is not in it");
    }

    /// A window of nothing holds nothing, including its own edge. Table 5
    /// gives a zero window its own row for exactly that reason.
    #[test]
    fn a_window_of_nothing_holds_nothing() {
        assert!(!Seq(1000).within(Seq(1000), 0));
    }

    /// A window that runs off the end of the space still works, which is the
    /// case a comparison written with `<` gets wrong.
    #[test]
    fn a_window_across_the_wrap_still_holds_what_is_in_it() {
        let start = Seq(u32::MAX - 4);
        assert!(start.within(start, 10));
        assert!(Seq(0).within(start, 10), "the wrap is inside the window");
        assert!(Seq(4).within(start, 10));
        assert!(!Seq(5).within(start, 10));
        assert!(!Seq(u32::MAX - 5).within(start, 10));
    }

    /// An acceptable acknowledgement, 3.4: SND.UNA < SEG.ACK =< SND.NXT.
    #[test]
    fn an_acknowledgement_is_acceptable_only_for_what_was_sent() {
        let (una, nxt) = (Seq(100), Seq(200));
        let acceptable = |ack: Seq| una.before(ack) && ack.before_or_at(nxt);
        assert!(!acceptable(una), "it acknowledged nothing new");
        assert!(acceptable(Seq(101)));
        assert!(acceptable(nxt), "it acknowledged everything sent");
        assert!(!acceptable(Seq(201)), "it acknowledged what was never sent");
        assert!(!acceptable(Seq(99)), "it went backwards");
    }
}
