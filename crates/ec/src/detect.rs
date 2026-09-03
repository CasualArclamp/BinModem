//! The V.42 detection phase (clause 7.2.1).
//!
//! Before any protocol runs, the two ends establish whether the far modem does
//! error control at all, by exchanging patterns of async-framed characters over
//! the synchronous bit stream.
//!
//! The patterns are more legible than their bit strings suggest. The originator
//! sends DC1 with alternating even and odd parity; the answerer replies with
//! `E` then `C`, for error control, or `E` then NUL to decline.

use std::collections::VecDeque;

/// Detection phase timer, 750 ms (V.42 9.1.1).
pub const DEFAULT_T400_MS: u32 = 750;

/// DC1 with even parity: the first character of the ODP (V.42 7.2.1.2).
pub const ODP_EVEN: u8 = 0x11;
/// DC1 with odd parity, the second character.
pub const ODP_ODD: u8 = 0x91;

/// `E`, the first character of every ADP (V.42 Table 3).
pub const ADP_E: u8 = b'E';
/// `C`, which together with `E` reports V.42 support.
pub const ADP_C: u8 = b'C';
/// NUL, which together with `E` declines error control.
pub const ADP_NULL: u8 = 0x00;

/// Ones between characters. V.42 permits 8 to 16; the middle is a safe choice.
const FILL_ONES: usize = 12;

/// What the answerer is reporting (V.42 Table 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// V.42 supported: proceed to protocol establishment.
    ErrorControl,
    /// No error-correcting protocol desired.
    None,
    /// One of the fifteen code points reserved for future assignment.
    Reserved(u8),
}

/// How the detection phase ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Still running.
    Pending,
    /// The far end answered.
    Answered(Answer),
    /// The originator's pattern was seen, so the far end does error control.
    OriginatorDetected,
    /// T400 elapsed with nothing recognised (V.42 7.2.1.2, 7.2.1.3).
    TimedOut,
}

/// Recovers async-framed characters from a synchronous bit stream.
///
/// The detection patterns are sent as start-stop characters even though the
/// link is synchronous at this point, which is why this cannot simply read
/// octets off the wire.
#[derive(Debug, Default)]
struct CharacterScanner {
    /// Bits collected since the start bit, or `None` while idle.
    collecting: Option<(u8, u32)>,
}

impl CharacterScanner {
    fn feed(&mut self, bit: bool) -> Option<u8> {
        match self.collecting {
            None => {
                // A space on an idle line is a start bit.
                if !bit {
                    self.collecting = Some((0, 0));
                }
                None
            }
            Some((value, count)) if count < 8 => {
                // Data bits, low-order first (V.42 Table 3 note).
                let value = value | (u8::from(bit) << count);
                self.collecting = Some((value, count + 1));
                None
            }
            Some((value, _)) => {
                self.collecting = None;
                // A framing error means this was not one of our characters.
                bit.then_some(value)
            }
        }
    }
}

/// Render one character as start bit, eight data bits and stop bit, followed by
/// the interval of ones that separates it from the next.
fn push_character(bits: &mut VecDeque<bool>, value: u8) {
    bits.push_back(false);
    for i in 0..8 {
        bits.push_back(value & (1 << i) != 0);
    }
    bits.push_back(true);
    for _ in 0..FILL_ONES {
        bits.push_back(true);
    }
}

/// The originator's side of the detection phase (V.42 7.2.1.2).
#[derive(Debug)]
pub struct Originator {
    out: VecDeque<bool>,
    scanner: CharacterScanner,
    /// Characters recognised from the answerer, in order.
    seen: Vec<u8>,
    /// Complete ADPs observed. Two adjacent ones are required.
    adps: Vec<Answer>,
    elapsed: u32,
    t400_ms: u32,
    outcome: Outcome,
}

impl Default for Originator {
    fn default() -> Self {
        Self::new(DEFAULT_T400_MS)
    }
}

impl Originator {
    pub fn new(t400_ms: u32) -> Self {
        let mut me = Self {
            out: VecDeque::new(),
            scanner: CharacterScanner::default(),
            seen: Vec::new(),
            adps: Vec::new(),
            elapsed: 0,
            t400_ms,
            outcome: Outcome::Pending,
        };
        me.queue_odp();
        me
    }

    /// Queue one repetition of the originator detection pattern.
    fn queue_odp(&mut self) {
        push_character(&mut self.out, ODP_EVEN);
        push_character(&mut self.out, ODP_ODD);
    }

    /// The next bit to transmit. The pattern repeats until detection ends.
    pub fn transmit(&mut self) -> bool {
        if self.outcome != Outcome::Pending {
            // Once detection is over the line idles at mark.
            return true;
        }
        if self.out.is_empty() {
            self.queue_odp();
        }
        self.out.pop_front().unwrap_or(true)
    }

    /// Feed one received bit.
    pub fn receive(&mut self, bit: bool) -> Outcome {
        if self.outcome != Outcome::Pending {
            return self.outcome;
        }
        if let Some(c) = self.scanner.feed(bit) {
            self.seen.push(c);
            self.classify();
        }
        self.outcome
    }

    /// Look for `E` followed by a type character, twice over.
    fn classify(&mut self) {
        let n = self.seen.len();
        if n < 2 {
            return;
        }
        if self.seen[n - 2] != ADP_E {
            return;
        }
        let answer = match self.seen[n - 1] {
            ADP_C => Answer::ErrorControl,
            ADP_NULL => Answer::None,
            other => Answer::Reserved(other),
        };
        self.adps.push(answer);
        // V.42 7.2.1.2: characters from at least two adjacent ADPs are needed
        // before the pattern counts as observed.
        if self.adps.len() >= 2 {
            self.outcome = Outcome::Answered(answer);
        }
    }

    /// Advance the detection timer (V.42 9.1.1).
    pub fn tick(&mut self, dt_ms: u32) -> Outcome {
        if self.outcome != Outcome::Pending {
            return self.outcome;
        }
        self.elapsed += dt_ms;
        if self.elapsed >= self.t400_ms {
            self.outcome = Outcome::TimedOut;
        }
        self.outcome
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }
}

/// The answerer's side of the detection phase (V.42 7.2.1.3).
#[derive(Debug)]
pub struct Answerer {
    out: VecDeque<bool>,
    scanner: CharacterScanner,
    /// DC1s of alternating parity seen so far.
    dc1_run: u32,
    last_parity: Option<u8>,
    elapsed: u32,
    t400_ms: u32,
    outcome: Outcome,
    answer: Answer,
}

impl Default for Answerer {
    fn default() -> Self {
        Self::new(DEFAULT_T400_MS, Answer::ErrorControl)
    }
}

impl Answerer {
    pub fn new(t400_ms: u32, answer: Answer) -> Self {
        Self {
            out: VecDeque::new(),
            scanner: CharacterScanner::default(),
            dc1_run: 0,
            last_parity: None,
            elapsed: 0,
            t400_ms,
            outcome: Outcome::Pending,
            answer,
        }
    }

    /// The next bit to transmit.
    ///
    /// V.42 7.2.1.3: the answerer sends marks until it recognises the ODP, then
    /// its own pattern.
    pub fn transmit(&mut self) -> bool {
        self.out.pop_front().unwrap_or(true)
    }

    fn queue_adp(&mut self) {
        let second = match self.answer {
            Answer::ErrorControl => ADP_C,
            Answer::None => ADP_NULL,
            Answer::Reserved(v) => v,
        };
        // Several repetitions, since the originator needs two adjacent ones.
        for _ in 0..4 {
            push_character(&mut self.out, ADP_E);
            push_character(&mut self.out, second);
        }
    }

    /// Feed one received bit.
    pub fn receive(&mut self, bit: bool) -> Outcome {
        if self.outcome != Outcome::Pending {
            return self.outcome;
        }
        let Some(c) = self.scanner.feed(bit) else {
            return self.outcome;
        };
        // V.42 7.2.1.3: at least four DC1s of alternating parity.
        if c == ODP_EVEN || c == ODP_ODD {
            if self.last_parity == Some(c) {
                // The same parity twice running breaks the alternation.
                self.dc1_run = 1;
            } else {
                self.dc1_run += 1;
            }
            self.last_parity = Some(c);
            if self.dc1_run >= 4 {
                self.outcome = Outcome::OriginatorDetected;
                self.queue_adp();
            }
        } else {
            self.dc1_run = 0;
            self.last_parity = None;
        }
        self.outcome
    }

    pub fn tick(&mut self, dt_ms: u32) -> Outcome {
        if self.outcome != Outcome::Pending {
            return self.outcome;
        }
        self.elapsed += dt_ms;
        if self.elapsed >= self.t400_ms {
            self.outcome = Outcome::TimedOut;
        }
        self.outcome
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// True once the answering pattern has been fully sent.
    pub fn finished_sending(&self) -> bool {
        self.outcome != Outcome::Pending && self.out.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run both ends against each other for at most `bits` bit times.
    fn exchange(o: &mut Originator, a: &mut Answerer, bits: usize) {
        for _ in 0..bits {
            let to_answerer = o.transmit();
            let to_originator = a.transmit();
            a.receive(to_answerer);
            o.receive(to_originator);
        }
    }

    #[test]
    fn the_odp_characters_are_dc1_with_alternating_parity() {
        // V.42 7.2.1.2: "0 1000 1000 1" and "0 1000 1001 1", low-order first.
        assert_eq!(ODP_EVEN, 0b0001_0001, "DC1 with an even parity bit");
        assert_eq!(ODP_ODD, 0b1001_0001, "DC1 with an odd parity bit");
        assert_eq!(ODP_EVEN & 0x7f, 0x11, "both are DC1");
        assert_eq!(ODP_ODD & 0x7f, 0x11);
        assert_eq!(ODP_EVEN.count_ones() % 2, 0);
        assert_eq!(ODP_ODD.count_ones() % 2, 1);
    }

    #[test]
    fn the_supported_adp_spells_error_control() {
        // V.42 Table 3: "0 1010 0010 1" and "0 1100 0010 1" are E and C.
        assert_eq!(ADP_E, 0x45);
        assert_eq!(ADP_C, 0x43);
        assert_eq!(&[ADP_E, ADP_C], b"EC");
    }

    #[test]
    fn a_character_round_trips_through_the_scanner() {
        let mut bits = VecDeque::new();
        push_character(&mut bits, ODP_EVEN);
        let mut scanner = CharacterScanner::default();
        let mut got = None;
        for b in bits {
            if let Some(c) = scanner.feed(b) {
                got = Some(c);
            }
        }
        assert_eq!(got, Some(ODP_EVEN));
    }

    #[test]
    fn every_byte_survives_framing() {
        for value in 0..=255u8 {
            let mut bits = VecDeque::new();
            push_character(&mut bits, value);
            let mut scanner = CharacterScanner::default();
            let mut got = None;
            for b in bits {
                if let Some(c) = scanner.feed(b) {
                    got = Some(c);
                }
            }
            assert_eq!(got, Some(value), "byte {value:#04x}");
        }
    }

    #[test]
    fn two_error_correcting_modems_find_each_other() {
        let mut o = Originator::default();
        let mut a = Answerer::default();
        exchange(&mut o, &mut a, 4000);
        assert_eq!(a.outcome(), Outcome::OriginatorDetected);
        assert_eq!(o.outcome(), Outcome::Answered(Answer::ErrorControl));
    }

    #[test]
    fn an_answerer_can_decline_error_control() {
        let mut o = Originator::default();
        let mut a = Answerer::new(DEFAULT_T400_MS, Answer::None);
        exchange(&mut o, &mut a, 4000);
        assert_eq!(o.outcome(), Outcome::Answered(Answer::None));
    }

    #[test]
    fn a_reserved_answer_is_reported_rather_than_guessed_at() {
        // V.42 Table 3 leaves fifteen code points for future assignment, so an
        // unknown one must not be read as either yes or no.
        let mut o = Originator::default();
        let mut a = Answerer::new(DEFAULT_T400_MS, Answer::Reserved(0x07));
        exchange(&mut o, &mut a, 4000);
        assert_eq!(o.outcome(), Outcome::Answered(Answer::Reserved(0x07)));
    }

    #[test]
    fn a_silent_far_end_times_out() {
        // A modem with no error control sends nothing recognisable.
        let mut o = Originator::default();
        for _ in 0..4000 {
            o.transmit();
            o.receive(true); // idle mark
        }
        assert_eq!(o.outcome(), Outcome::Pending);
        assert_eq!(o.tick(DEFAULT_T400_MS), Outcome::TimedOut);
    }

    #[test]
    fn an_answerer_hearing_nothing_times_out() {
        let mut a = Answerer::default();
        for _ in 0..4000 {
            a.transmit();
            a.receive(true);
        }
        assert_eq!(a.tick(DEFAULT_T400_MS), Outcome::TimedOut);
    }

    #[test]
    fn the_timer_does_not_fire_early() {
        let mut o = Originator::new(750);
        assert_eq!(o.tick(700), Outcome::Pending);
        assert_eq!(o.tick(50), Outcome::TimedOut);
    }

    #[test]
    fn four_alternating_dc1s_are_required() {
        // V.42 7.2.1.3. The same parity repeated is not alternation.
        let mut a = Answerer::default();
        let mut bits = VecDeque::new();
        for _ in 0..6 {
            push_character(&mut bits, ODP_EVEN);
        }
        for b in bits {
            a.receive(b);
        }
        assert_eq!(
            a.outcome(),
            Outcome::Pending,
            "repeating one parity should not count as the ODP"
        );
    }

    #[test]
    fn unrelated_traffic_does_not_trigger_detection() {
        let mut a = Answerer::default();
        let mut bits = VecDeque::new();
        for c in b"hello there, this is not a detection pattern" {
            push_character(&mut bits, *c);
        }
        for b in bits {
            a.receive(b);
        }
        assert_eq!(a.outcome(), Outcome::Pending);
    }

    #[test]
    fn one_adp_is_not_enough_for_the_originator() {
        // V.42 7.2.1.2 requires characters from two adjacent ADPs.
        let mut o = Originator::default();
        let mut bits = VecDeque::new();
        push_character(&mut bits, ADP_E);
        push_character(&mut bits, ADP_C);
        for b in bits {
            o.receive(b);
        }
        assert_eq!(o.outcome(), Outcome::Pending);

        let mut more = VecDeque::new();
        push_character(&mut more, ADP_E);
        push_character(&mut more, ADP_C);
        for b in more {
            o.receive(b);
        }
        assert_eq!(o.outcome(), Outcome::Answered(Answer::ErrorControl));
    }

    #[test]
    fn the_answerer_idles_at_mark_before_detecting() {
        // V.42 7.2.1.3: marks until the ODP is recognised.
        let mut a = Answerer::default();
        for _ in 0..100 {
            assert!(a.transmit(), "answerer should idle at mark");
        }
    }

    #[test]
    fn the_originator_stops_sending_once_answered() {
        let mut o = Originator::default();
        let mut a = Answerer::default();
        exchange(&mut o, &mut a, 4000);
        assert_eq!(o.outcome(), Outcome::Answered(Answer::ErrorControl));
        for _ in 0..50 {
            assert!(o.transmit(), "should idle at mark after detection");
        }
    }
}
