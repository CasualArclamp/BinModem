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
/// `M`, which reports a proprietary cellular protocol *and* V.42 support
/// (Appendix VI.1). Sent five or more times, and then `EC`.
pub const ADP_M: u8 = b'M';
/// `P`, which reports V.42 support and that the XID user data subfield may be
/// extended to carry V.44 parameters (Appendix VI.1). Sent sixteen times, and
/// then `EC`.
pub const ADP_P: u8 = b'P';

/// Ones between characters. V.42 permits 8 to 16; the middle is a safe choice.
const FILL_ONES: usize = 12;

/// Repetitions of the ADP before the answerer will consider stopping.
///
/// V.42 7.2.1.3 requires "at least ten times", and it is a floor rather than a
/// figure: the originator needs two *adjacent* patterns received correctly,
/// and Appendix III.1 points out that on a line bad enough to spoil most of
/// them the answerer should go on well past ten, because by then it has
/// already heard the ODP and so knows there is a V.42 modem listening.
const ADP_REPEATS: u32 = 10;

/// An HDLC flag, which is how the originator says the protocol phase has begun
/// (V.42 7.2.1.3) and so the ADP has been heard.
///
/// The same byte read either way round, so the bit order does not arise.
const FLAG: u8 = 0b0111_1110;

/// What the answerer is reporting (V.42 Table 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// V.42 supported: proceed to protocol establishment.
    ErrorControl,
    /// No error-correcting protocol desired.
    None,
    /// A pattern that reports V.42 support without being `EC`, and is sent
    /// *ahead* of `EC` rather than instead of it (Appendix VI.1).
    ///
    /// Table 3 describes two meanings and reserves fifteen code points, and
    /// the appendix observes drily that "in actuality, there are more than 15
    /// other patterns". Two are documented: `EM` from a cellular protocol, and
    /// `EP`, which says the XID user data subfield may carry V.44. Both are
    /// followed by the ordinary `EC`, so the thing to do on seeing one is to
    /// go on listening.
    Extended(u8),
    /// One of the fifteen code points reserved for future assignment.
    Reserved(u8),
}

impl Answer {
    /// Read the second character of an ADP.
    fn from_char(c: u8) -> Self {
        match c {
            ADP_C => Self::ErrorControl,
            ADP_NULL => Self::None,
            ADP_M | ADP_P => Self::Extended(c),
            other => Self::Reserved(other),
        }
    }

    /// Whether the far end has said it does V.42.
    ///
    /// The extended patterns say so as plainly as `EC` does; what they add is
    /// something about *how*, which is no reason to hear them as a refusal.
    pub fn error_controlled(self) -> bool {
        matches!(self, Self::ErrorControl | Self::Extended(_))
    }

    /// Whether the far end offered V.44 in the XID user data subfield.
    pub fn offers_v44(self) -> bool {
        self == Self::Extended(ADP_P)
    }
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
    ///
    /// An extended pattern is recorded and listened past rather than acted on.
    /// Appendix VI.1 describes real modems that send `EM` five times or `EP`
    /// sixteen times *before* the `EC` that says what they support, and a
    /// detector that stops at the first pattern it sees would answer one of
    /// those by declining error control to a modem that has it. Nothing else
    /// is treated this way: 7.2.1.2 says to act on the ADP received, and an
    /// unknown reserved code point is not documented as a prefix to anything.
    fn classify(&mut self) {
        let n = self.seen.len();
        if n < 2 {
            return;
        }
        if self.seen[n - 2] != ADP_E {
            return;
        }
        let answer = Answer::from_char(self.seen[n - 1]);
        self.adps.push(answer);
        // V.42 7.2.1.2: characters from at least two adjacent ADPs are needed
        // before the pattern counts as observed.
        if self.adps.len() >= 2 && !matches!(answer, Answer::Extended(_)) {
            self.outcome = Outcome::Answered(answer);
        }
    }

    /// Every complete ADP seen, in order.
    ///
    /// The last one is the answer; the ones before it are what the far end
    /// said about itself on the way there.
    pub fn patterns(&self) -> &[Answer] {
        &self.adps
    }

    /// Advance the detection timer (V.42 9.1.1).
    pub fn tick(&mut self, dt_ms: u32) -> Outcome {
        if self.outcome != Outcome::Pending {
            return self.outcome;
        }
        self.elapsed += dt_ms;
        if self.elapsed >= self.t400_ms {
            // A pattern that says V.42 without saying `EC` still says V.42
            // (Appendix VI.1). If the `EC` that should have followed was lost
            // to the line, what was heard before it is not nothing.
            self.outcome = match self.adps.last() {
                Some(&a) if self.adps.len() >= 2 && a.error_controlled() => Outcome::Answered(a),
                _ => Outcome::TimedOut,
            };
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
    /// Repetitions of the ADP queued so far.
    repeats: u32,
    /// The last eight bits from the line, watched for a flag.
    history: u8,
    /// Whether the originator has begun the protocol phase (7.2.1.3).
    flags: bool,
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
            repeats: 0,
            history: 0,
            flags: false,
        }
    }

    /// The next bit to transmit.
    ///
    /// V.42 7.2.1.3: the answerer sends marks until it recognises the ODP, then
    /// its own pattern, until the originator's flags say it was heard.
    pub fn transmit(&mut self) -> bool {
        if self.out.is_empty() && self.outcome == Outcome::OriginatorDetected && !self.said_enough()
        {
            self.queue_adp();
        }
        self.out.pop_front().unwrap_or(true)
    }

    /// Whether the pattern has been sent for long enough to stop.
    ///
    /// Ten repetitions is the floor 7.2.1.3 sets. Past that the answerer keeps
    /// going until the originator's flags arrive, which is Appendix III.1's
    /// advice and costs nothing, because there is nothing else for the line to
    /// be carrying in the meantime. The clock bounds it for the case where the
    /// flags never come at all -- a far end that heard the ODP out of noise it
    /// made itself, and is not in fact a modem doing V.42.
    fn said_enough(&self) -> bool {
        self.repeats >= ADP_REPEATS && (self.flags || self.elapsed >= self.t400_ms)
    }

    fn queue_adp(&mut self) {
        let second = match self.answer {
            Answer::ErrorControl => ADP_C,
            Answer::None => ADP_NULL,
            Answer::Extended(v) | Answer::Reserved(v) => v,
        };
        push_character(&mut self.out, ADP_E);
        push_character(&mut self.out, second);
        self.repeats += 1;
    }

    /// Feed one received bit.
    pub fn receive(&mut self, bit: bool) -> Outcome {
        // Watched past the end of detection, because what stops the answerer's
        // pattern is the originator's flags (7.2.1.3) and those arrive after
        // the ODP has already been recognised.
        self.history = (self.history << 1) | u8::from(bit);
        if self.outcome == Outcome::OriginatorDetected && self.history == FLAG {
            self.flags = true;
        }
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
                // The clock restarts, so that the bound on how long the
                // pattern is repeated is a bound on the repeating rather than
                // whatever was left of the detection timer.
                self.elapsed = 0;
                self.queue_adp();
            }
        } else {
            self.dc1_run = 0;
            self.last_parity = None;
        }
        self.outcome
    }

    pub fn tick(&mut self, dt_ms: u32) -> Outcome {
        self.elapsed += dt_ms;
        if self.outcome != Outcome::Pending {
            return self.outcome;
        }
        if self.elapsed >= self.t400_ms {
            self.outcome = Outcome::TimedOut;
        }
        self.outcome
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }

    /// True once the answering pattern has been sent for as long as it should
    /// be, and the last repetition is out.
    pub fn finished_sending(&self) -> bool {
        self.outcome != Outcome::Pending && self.out.is_empty() && self.said_enough()
    }

    /// Whether the originator has started the protocol phase.
    pub fn heard_flags(&self) -> bool {
        self.flags
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

    /// Feed whole ADPs into an originator, as bits.
    fn say(o: &mut Originator, second: u8, times: usize) {
        for _ in 0..times {
            let mut bits = VecDeque::new();
            push_character(&mut bits, ADP_E);
            push_character(&mut bits, second);
            for b in bits {
                o.receive(b);
            }
        }
    }

    #[test]
    fn the_extended_patterns_spell_out_their_letters() {
        // Appendix VI.1: "0 1011 0010 1" and "0 0000 1010 1", low-order first.
        assert_eq!(ADP_M, 0b0100_1101);
        assert_eq!(ADP_P, 0b0101_0000);
        assert_eq!(&[ADP_E, ADP_M], b"EM");
        assert_eq!(&[ADP_E, ADP_P], b"EP");
        // Table 3 reserves "0 0000 XXXX", which sends its low nibble first and
        // so is a multiple of sixteen. `P` is one of those code points, which
        // is why the appendix calls it "the previously reserved pattern"; `M`
        // is not one at all, which is why it says there are more than fifteen.
        assert_eq!(ADP_P % 16, 0, "P is one of the reserved code points");
        assert_ne!(ADP_M % 16, 0, "M was never a code point in Table 3");
    }

    #[test]
    fn a_cellular_modem_saying_em_first_still_gets_error_control() {
        // Appendix VI.1: EM five or more times, then EC ten or more. Stopping
        // at the first pattern would decline error control to a modem that has
        // it, which is the whole point of the appendix.
        let mut o = Originator::default();
        say(&mut o, ADP_M, 5);
        assert_eq!(o.outcome(), Outcome::Pending, "EM is a prefix, not an answer");
        say(&mut o, ADP_C, 10);
        assert_eq!(o.outcome(), Outcome::Answered(Answer::ErrorControl));
    }

    #[test]
    fn a_modem_offering_v44_says_ep_sixteen_times_first() {
        // Appendix VI.1: EP sixteen times, then EC. Sixteen is a lot of
        // patterns to sit through without concluding anything.
        let mut o = Originator::default();
        say(&mut o, ADP_P, 16);
        assert_eq!(o.outcome(), Outcome::Pending);
        say(&mut o, ADP_C, 10);
        assert_eq!(o.outcome(), Outcome::Answered(Answer::ErrorControl));
        assert!(
            o.patterns().iter().any(|a| a.offers_v44()),
            "the far end said its XID may carry V.44"
        );
    }

    #[test]
    fn an_extended_pattern_alone_is_still_error_control() {
        // If the EC that should have followed is lost, what came before it is
        // not nothing: EM and EP both report V.42 support in their own right.
        let mut o = Originator::default();
        say(&mut o, ADP_M, 5);
        assert_eq!(o.tick(DEFAULT_T400_MS), Outcome::Answered(Answer::Extended(ADP_M)));
        assert!(matches!(o.outcome(), Outcome::Answered(a) if a.error_controlled()));
    }

    #[test]
    fn an_unknown_pattern_is_not_listened_past() {
        // 7.2.1.2 says to act on the ADP received. Only the two patterns the
        // appendix documents as prefixes are treated as prefixes; a reserved
        // code point nobody has described is an answer like any other.
        let mut o = Originator::default();
        say(&mut o, 0x20, 2);
        assert_eq!(o.outcome(), Outcome::Answered(Answer::Reserved(0x20)));
    }

    /// Count the ADPs an answerer sends over a run of bit times.
    fn adps_sent(a: &mut Answerer, bits: usize) -> usize {
        let mut scanner = CharacterScanner::default();
        let mut seen = Vec::new();
        for _ in 0..bits {
            if let Some(c) = scanner.feed(a.transmit()) {
                seen.push(c);
            }
        }
        seen.windows(2).filter(|w| w[0] == ADP_E && w[1] == ADP_C).count()
    }

    #[test]
    fn the_answerer_sends_the_pattern_at_least_ten_times() {
        // V.42 7.2.1.3: "at least ten times". Four is not ten, and the
        // originator needs two of them adjacent and undamaged.
        let mut a = Answerer::default();
        let mut o = Originator::default();
        for _ in 0..400 {
            a.receive(o.transmit());
        }
        assert_eq!(a.outcome(), Outcome::OriginatorDetected);
        assert!(adps_sent(&mut a, 20_000) >= 10);
    }

    #[test]
    fn the_answerer_stops_when_the_protocol_phase_begins() {
        // V.42 7.2.1.3: the pattern ends on "receipt of continuous flags".
        let mut a = Answerer::default();
        let mut o = Originator::default();
        for _ in 0..400 {
            a.receive(o.transmit());
        }
        // Ten repetitions first, whatever else happens.
        for _ in 0..20_000 {
            a.transmit();
        }
        assert!(!a.heard_flags());
        assert!(!a.finished_sending(), "no flags and no timer: keep going");
        for byte in [FLAG; 4] {
            for i in (0..8).rev() {
                a.receive(byte & (1 << i) != 0);
            }
        }
        assert!(a.heard_flags());
        for _ in 0..2000 {
            a.transmit();
        }
        assert!(a.finished_sending());
        for _ in 0..200 {
            assert!(a.transmit(), "mark once the pattern is done");
        }
    }

    #[test]
    fn an_originator_that_never_speaks_does_not_hold_the_answerer_for_ever() {
        // The far end heard an ODP out of noise it made itself and is not in
        // fact a V.42 modem. The clock is what ends that.
        let mut a = Answerer::default();
        let mut o = Originator::default();
        for _ in 0..400 {
            a.receive(o.transmit());
        }
        for _ in 0..20_000 {
            a.transmit();
        }
        assert!(!a.finished_sending());
        a.tick(DEFAULT_T400_MS);
        for _ in 0..2000 {
            a.transmit();
        }
        assert!(a.finished_sending());
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
