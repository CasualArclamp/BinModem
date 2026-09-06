//! V.8: finding out what the other modem is before trying to talk to it.
//!
//! Every modem Recommendation describes a start-up, and every one of those
//! start-ups assumes both ends already agree which Recommendation is being
//! followed. Nothing in V.32 tells an answering modem that the caller wanted
//! V.32; it simply begins, and if the far end is doing something else the two
//! sit there transmitting past each other until one gives up. That failure
//! looks, from the calling end, exactly like a modem that never answered.
//!
//! V.8 is the conversation that happens first. The answering modem sends a
//! tone saying "I can do this"; the calling modem answers with a list of what
//! it has; the answering modem replies with the ones they share; and both then
//! jump straight into the start-up of whichever they picked. Clause 1: "a
//! means to determine automatically, prior to initiation of modem handshake,
//! the best available operational mode between two DCEs".
//!
//! What is here is the messages -- the octets of CM, JM, CI and CJ, and the
//! rule that turns two menus into one choice. It owns no line and no timers:
//! the messages ride on V.21 at 300 bit/s, framed as ordinary start-stop
//! octets, and that is somebody else's problem. Which means the whole of the
//! negotiation can be tested without a modem anywhere near it.
//!
//! Bit order throughout is the Recommendation's: the tables read
//! `Start b0 b1 b2 b3 b4 b5 b6 b7 Stop`, so `b0` is the first data bit after
//! the start bit, which is the least significant bit of the octet an
//! asynchronous framer hands over.

#![forbid(unsafe_code)]

pub mod ansam;

pub use ansam::AnswerTone;

/// The ten ONEs every sequence opens with (Table 1).
///
/// Not an octet. It is the idle condition of the line held for ten bit times,
/// which is what a receiver needs in order to find the first start bit that
/// follows.
pub const PREAMBLE_ONES: usize = 10;

/// Synchronisation for CI, as an octet (Table 1: `0000000001`).
///
/// Ten bits, which is a start bit, eight zeros, and a stop bit -- so it is
/// carried as an ordinary framed octet whose value happens to be zero.
pub const SYNC_CI: u8 = 0x00;

/// Synchronisation for CM and JM (Table 1: `0000001111`).
///
/// Start bit, then `b0..b7` = `00000111`, then the stop bit. With `b0` least
/// significant that octet is `0xE0`.
pub const SYNC_MENU: u8 = 0xE0;

/// What the call is for (Table 3), in the three option bits `b5 b6 b7`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallFunction {
    /// `100`, ITU-T H.324.
    MultimediaTerminal,
    /// `010`, ITU-T V.18.
    Textphone,
    /// `110`, ITU-T T.101.
    Videotext,
    /// `001`, ITU-T T.30.
    TransmitFax,
    /// `101`, ITU-T T.30.
    ReceiveFax,
    /// `011`. What a modem calling a bulletin board is doing.
    Data,
}

impl CallFunction {
    /// The three option bits, `b5` first.
    const fn bits(self) -> [bool; 3] {
        match self {
            Self::MultimediaTerminal => [true, false, false],
            Self::Textphone => [false, true, false],
            Self::Videotext => [true, true, false],
            Self::TransmitFax => [false, false, true],
            Self::ReceiveFax => [true, false, true],
            Self::Data => [false, true, true],
        }
    }

    fn from_bits(bits: [bool; 3]) -> Option<Self> {
        // `000` is reserved and `111` says the function is in an extension
        // octet, which nothing here sends and nothing here has to read: 5
        // says a receiver shall ignore what is reserved for future
        // definition, and an extension it cannot interpret is exactly that.
        [
            Self::MultimediaTerminal,
            Self::Textphone,
            Self::Videotext,
            Self::TransmitFax,
            Self::ReceiveFax,
            Self::Data,
        ]
        .into_iter()
        .find(|f| f.bits() == bits)
    }
}

/// A modulation V.8 has a bit for (Table 4).
///
/// The order is the order of the bits in the table, which is also the order of
/// its item numbers, which is also the order of preference: 7.4 settles a
/// negotiation by taking "the indicated modulation category modulation mode
/// with the lowest item number".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Modulation {
    V34Duplex,
    V34HalfDuplex,
    /// One bit covers both: "V.32 bis/V.32 availability".
    V32bis,
    /// Likewise "V.22 bis/V.22 availability".
    V22bis,
    V17,
    V29HalfDuplex,
    V27ter,
    V26ter,
    V26bis,
    V23Duplex,
    V23HalfDuplex,
    V21,
}

impl Modulation {
    /// Every modulation, in item order.
    pub const ALL: [Self; 12] = [
        Self::V34Duplex,
        Self::V34HalfDuplex,
        Self::V32bis,
        Self::V22bis,
        Self::V17,
        Self::V29HalfDuplex,
        Self::V27ter,
        Self::V26ter,
        Self::V26bis,
        Self::V23Duplex,
        Self::V23HalfDuplex,
        Self::V21,
    ];

    /// Which of the three octets carries it, and which bit of that octet.
    ///
    /// The gaps are not free space. `modn0` spends `b0..b4` on the category
    /// tag and `b5` on whether a PCM category follows; `modn1` and `modn2`
    /// spend `b3..b5` on the code that marks them as extension octets. Only
    /// what is left holds modulations.
    const fn place(self) -> (usize, u8) {
        match self {
            Self::V34Duplex => (0, 6),
            Self::V34HalfDuplex => (0, 7),
            Self::V32bis => (1, 0),
            Self::V22bis => (1, 1),
            Self::V17 => (1, 2),
            Self::V29HalfDuplex => (1, 6),
            Self::V27ter => (1, 7),
            Self::V26ter => (2, 0),
            Self::V26bis => (2, 1),
            Self::V23Duplex => (2, 2),
            Self::V23HalfDuplex => (2, 6),
            Self::V21 => (2, 7),
        }
    }

    /// What to call it.
    pub fn name(self) -> &'static str {
        match self {
            Self::V34Duplex => "V.34",
            Self::V34HalfDuplex => "V.34 half-duplex",
            Self::V32bis => "V.32bis/V.32",
            Self::V22bis => "V.22bis/V.22",
            Self::V17 => "V.17",
            Self::V29HalfDuplex => "V.29 half-duplex",
            Self::V27ter => "V.27ter",
            Self::V26ter => "V.26ter",
            Self::V26bis => "V.26bis",
            Self::V23Duplex => "V.23",
            Self::V23HalfDuplex => "V.23 half-duplex",
            Self::V21 => "V.21",
        }
    }
}

/// A set of modulations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modulations(u16);

impl Modulations {
    pub const NONE: Self = Self(0);

    pub fn of(list: &[Modulation]) -> Self {
        let mut set = Self::NONE;
        for &m in list {
            set.insert(m);
        }
        set
    }

    pub fn insert(&mut self, m: Modulation) {
        self.0 |= 1 << Self::index(m);
    }

    pub fn contains(self, m: Modulation) -> bool {
        self.0 & (1 << Self::index(m)) != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// What both ends have.
    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// The one to use: 7.4, "the indicated modulation category modulation mode
    /// with the lowest item number".
    ///
    /// The item numbers run down Table 4 from V.34 to V.21, so the lowest is
    /// also the fastest, and this is a preference order rather than an
    /// arbitrary one.
    pub fn best(self) -> Option<Modulation> {
        Modulation::ALL.into_iter().find(|&m| self.contains(m))
    }

    pub fn iter(self) -> impl Iterator<Item = Modulation> {
        Modulation::ALL.into_iter().filter(move |&m| self.contains(m))
    }

    fn index(m: Modulation) -> u16 {
        Modulation::ALL.iter().position(|&x| x == m).expect("in ALL") as u16
    }
}

/// What the protocol category says (Table 6).
///
/// The category exists to settle error control before the line has been
/// trained, and 7.3 says why anyone would bother: it "may be included in order
/// to negotiate LAPM without requiring the ODP/ADP exchange".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    /// No protocol octet. Table 6's note is explicit that this settles
    /// nothing: "absence of this octet does not preclude alternative means of
    /// protocol negotiation" -- and the V.42 detection phase is one.
    #[default]
    Unstated,
    /// LAPM, according to ITU-T V.42.
    Lapm,
    /// A protocol named in an extension octet. Read only far enough to know it
    /// is not the one this modem does.
    Extended,
}

/// The contents of a CM or a JM: what the call is for, and what can carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Menu {
    pub function: CallFunction,
    pub modulations: Modulations,
    /// What error control the far end will be asked for, if anything.
    pub protocol: Protocol,
}

/// Category tags, in bits `b0..b3` of a category octet (Table 2).
mod tag {
    // Written as numbers, not as the table writes them, and the two are
    // mirror images. Table 2 lists the call function tag as `1000` because it
    // prints b0 first and b0 goes on the line first -- but b0 is the least
    // significant bit of the octet, so that same tag is the number 1. Reading
    // the table straight into a constant gets every tag backwards, and the
    // ones that are palindromes would still have worked, which is worse.
    /// `b0 b1 b2 b3` = `1 0 0 0`.
    pub const CALL_FUNCTION: u8 = 0b0001;
    /// `b0 b1 b2 b3` = `1 0 1 0`.
    pub const MODULATION: u8 = 0b0101;
    /// `b0 b1 b2 b3` = `0 1 0 1`, which is the modulation tag backwards and so
    /// a free check that the bit order here is the right way round.
    pub const PROTOCOL: u8 = 0b1010;
}

/// Build an octet from its bits, `b0` least significant.
fn octet(bits: [bool; 8]) -> u8 {
    bits.iter()
        .enumerate()
        .fold(0u8, |acc, (i, &b)| acc | (u8::from(b) << i))
}

/// Read bit `n` of an octet, numbered as the tables number them.
fn bit(o: u8, n: u8) -> bool {
    o & (1 << n) != 0
}

/// The four tag bits of a category octet.
fn tag_of(o: u8) -> u8 {
    o & 0b1111
}

impl Menu {
    /// The category octets of this menu, in the order they go on the line.
    ///
    /// The call function first -- 7.3, "the first information category in CM
    /// indicates the required call function" -- then all three modulation
    /// octets. All three, always: the later ones are reached by the extension
    /// code in the middle of each, so there is no way to send the third
    /// without the second, and a menu offering only V.21 needs the third.
    pub fn octets(&self) -> Vec<u8> {
        let f = self.function.bits();
        let mut out = vec![octet([
            true, false, false, false, // b0..b3: the call function tag, 1000
            false, // b4: a category octet
            f[0], f[1], f[2],
        ])];

        let has = |m: Modulation| self.modulations.contains(m);
        out.push(octet([
            true, false, true, false, // b0..b3: the modulation tag, 1010
            false, // b4: a category octet
            false, // b5: no PCM modem category follows
            has(Modulation::V34Duplex),
            has(Modulation::V34HalfDuplex),
        ]));
        out.push(octet([
            has(Modulation::V32bis),
            has(Modulation::V22bis),
            has(Modulation::V17),
            false,
            true,
            false, // b3..b5: 010, marking an extension octet
            has(Modulation::V29HalfDuplex),
            has(Modulation::V27ter),
        ]));
        out.push(octet([
            has(Modulation::V26ter),
            has(Modulation::V26bis),
            has(Modulation::V23Duplex),
            false,
            true,
            false,
            has(Modulation::V23HalfDuplex),
            has(Modulation::V21),
        ]));

        // Last, because 7.3 gives an order for the two categories before it
        // and none for this one, and because a reader that does not know the
        // category has to skip it -- which is easiest at the end.
        if self.protocol == Protocol::Lapm {
            out.push(octet([
                false, true, false, true, // b0..b3: the protocol tag, 0101
                false, // b4: a category octet
                true, false, false, // b5..b7: 100, LAPM per ITU-T V.42
            ]));
        }
        out
    }

    /// Read a menu back out of the octets that followed a sync.
    ///
    /// Categories this does not know are skipped rather than refused. Clause 5:
    /// "a receiver shall ignore all bits, codes and octets reserved for such
    /// future definition" -- and the Recommendation says plainly that it is
    /// designed to be extensible, so a menu carrying a category invented after
    /// this was written is a menu to read the rest of, not one to throw away.
    pub fn parse(octets: &[u8]) -> Option<Self> {
        let mut function = None;
        let mut modulations = Modulations::NONE;
        let mut protocol = Protocol::Unstated;
        let mut rest = octets.iter().copied().peekable();
        while let Some(o) = rest.next() {
            // Only category octets carry a tag; an extension octet is
            // identified by its own contents and is consumed by whichever
            // category claimed it.
            match tag_of(o) {
                tag::CALL_FUNCTION if !bit(o, 4) => {
                    function =
                        CallFunction::from_bits([bit(o, 5), bit(o, 6), bit(o, 7)]);
                }
                tag::MODULATION if !bit(o, 4) => {
                    let mut group = [o, 0, 0];
                    // The two extension octets, if they were sent. A menu may
                    // stop after any of the three.
                    for slot in group.iter_mut().skip(1) {
                        match rest.peek() {
                            Some(&next) if is_extension(next) => {
                                *slot = next;
                                rest.next();
                            }
                            _ => break,
                        }
                    }
                    for m in Modulation::ALL {
                        let (which, b) = m.place();
                        if bit(group[which], b) {
                            modulations.insert(m);
                        }
                    }
                }
                tag::PROTOCOL if !bit(o, 4) => {
                    // Table 6 defines two codes in b5..b7 and reserves the
                    // rest. `111` names an extension octet this modem does not
                    // read; anything else is a code that did not exist when
                    // this was written, and clause 5 says to ignore it.
                    protocol = match (bit(o, 5), bit(o, 6), bit(o, 7)) {
                        (true, false, false) => Protocol::Lapm,
                        (true, true, true) => Protocol::Extended,
                        _ => Protocol::Unstated,
                    };
                }
                _ => {}
            }
        }
        Some(Self { function: function?, modulations, protocol })
    }

    /// The joint menu: what this end has that the far end also offered.
    ///
    /// 7.4: JM "shall include the octets necessary to indicate all modulation
    /// modes that are both indicated in CM and available in the answer DCE".
    /// And when there is nothing in common, the reply is not silence -- it is
    /// a menu with every modulation bit clear, which says so.
    pub fn joint(&self, ours: Modulations, protocol: Protocol) -> Self {
        Self {
            function: self.function,
            modulations: self.modulations.intersect(ours),
            // 7.4: "if the LAPM protocol code is indicated in CM, the protocol
            // octet may be included in JM in order to complete the
            // negotiation". Only then. An answer naming error control the call
            // never asked about is not a negotiation, it is an announcement,
            // and the calling modem has no reason to be reading it.
            protocol: match (self.protocol, protocol) {
                (Protocol::Lapm, Protocol::Lapm) => Protocol::Lapm,
                _ => Protocol::Unstated,
            },
        }
    }

    /// Whether both ends have now said LAPM.
    ///
    /// True of a JM, which is the joint menu and so already the intersection;
    /// true of a CM only in the sense that the calling modem asked.
    pub fn lapm(&self) -> bool {
        self.protocol == Protocol::Lapm
    }

    /// Which modulation the call will use, if the two ends found one.
    pub fn chosen(&self) -> Option<Modulation> {
        self.modulations.best()
    }
}

/// Whether an octet is one of the modulation category's extension octets.
///
/// They are told apart from a new category by the `010` in `b3..b5` where a
/// category octet has its tag and a zero.
fn is_extension(o: u8) -> bool {
    !bit(o, 3) && bit(o, 4) && !bit(o, 5)
}

/// Which of V.8's signals a sequence is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Call indicator: the calling modem saying what it is for, before any
    /// answer tone. Optional, and 7.1 requires a receiver not to malfunction
    /// on it whether or not it uses it.
    Ci,
    /// Call menu: what the calling modem can do.
    Cm,
    /// Joint menu: what both ends can do.
    Jm,
}

impl Signal {
    /// The synchronisation octet that opens this signal.
    pub fn sync(self) -> u8 {
        match self {
            Self::Ci => SYNC_CI,
            Self::Cm | Self::Jm => SYNC_MENU,
        }
    }
}

/// The octets of one whole sequence: the synchronisation, then the categories.
///
/// The ten ONEs in front of it are not octets and are not here; they are the
/// idle line, which the transmitter holds before it starts.
pub fn sequence(signal: Signal, menu: &Menu) -> Vec<u8> {
    let mut out = vec![signal.sync()];
    match signal {
        // 7.1: a CI sequence is the synchronisation and the call function
        // octet, and nothing else. It is announcing what the call is for, not
        // negotiating how to carry it.
        Signal::Ci => out.push(menu.octets()[0]),
        Signal::Cm | Signal::Jm => out.extend(menu.octets()),
    }
    out
}

/// The CM terminator, 3.5: "three consecutive octets of all ZEROs".
///
/// It acknowledges JM and ends CM. Zero is not a category tag, so it cannot be
/// mistaken for one.
pub const CJ: [u8; 3] = [0, 0, 0];

/// Reads V.8 sequences out of a stream of framed octets.
///
/// Fed whatever the asynchronous framer recovers. It finds a synchronisation
/// octet, gathers what follows, and reports a sequence once the next
/// synchronisation arrives or the sequence is repeated -- which is how these
/// are sent: 7.3, "a repetitive sequence of bits", over and over until the far
/// end answers.
#[derive(Debug, Default)]
pub struct Decoder {
    kind: Option<Signal>,
    body: Vec<u8>,
    zeros: usize,
}

/// What a decoder found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    Ci(CallFunction),
    Cm(Menu),
    Jm(Menu),
    /// Three zero octets in a row: the far end has seen our JM and is done.
    Cj,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// One framed octet. Returns a sequence when one completes.
    pub fn feed(&mut self, octet: u8) -> Option<Heard> {
        // CJ first, and regardless of what is being gathered: it is three
        // zeros wherever it lands, and it means the conversation is over.
        self.zeros = if octet == 0 { self.zeros + 1 } else { 0 };
        if self.zeros >= CJ.len() {
            self.zeros = 0;
            // A CI sync is also a zero octet, so a run of them is only a CJ if
            // we were not part-way into reading a CI.
            if self.kind != Some(Signal::Ci) {
                self.kind = None;
                self.body.clear();
                return Some(Heard::Cj);
            }
        }

        if octet == SYNC_MENU {
            // A menu is starting -- or repeating, which is the same event seen
            // from the far end of the last one. Either way what was being
            // gathered is finished, and is worth reporting if it parsed.
            //
            // The repeat is the terminator because a menu has no length. V.8
            // clause 5 requires a receiver to "ignore all bits, codes and
            // octets reserved for such future definition", which it can only
            // do if it is not counting them, and 7.3 and 7.4 both describe
            // categories that may or may not be there. This used to stop after
            // four octets, which was the length of every menu it could build
            // at the time -- and the fifth category, the protocol octet that
            // settles error control, fell off the end of every menu carrying
            // one. The synchronisation octet cannot be mistaken for a body
            // octet: its low nibble is zero and Table 2 gives no category that
            // tag, and it has a clear b4 where a modulation extension octet
            // has a set one.
            let done = self.finish();
            self.kind = Some(Signal::Cm);
            self.body.clear();
            return done;
        }
        if let Some(kind) = self.kind {
            self.body.push(octet);
            // A CI is one octet long and known to be complete at once.
            if kind == Signal::Ci && self.body.len() == 1 {
                let f = CallFunction::from_bits([
                    bit(octet, 5),
                    bit(octet, 6),
                    bit(octet, 7),
                ]);
                self.kind = None;
                self.body.clear();
                return f.map(Heard::Ci);
            }
        } else if octet == SYNC_CI {
            self.kind = Some(Signal::Ci);
            self.body.clear();
        }
        None
    }

    /// A decoder for the answering side, which hears JM rather than CM.
    ///
    /// The two are the same octets; which one it is depends on which V.21
    /// channel carried it, and that is the transport's business, not this
    /// one's. This only changes what the result is called.
    pub fn heard_as_jm(heard: Heard) -> Heard {
        match heard {
            Heard::Cm(menu) => Heard::Jm(menu),
            other => other,
        }
    }

    fn finish(&mut self) -> Option<Heard> {
        let body = std::mem::take(&mut self.body);
        self.kind = None;
        if body.is_empty() {
            return None;
        }
        Menu::parse(&body).map(Heard::Cm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_menu(list: &[Modulation]) -> Menu {
        Menu {
            function: CallFunction::Data,
            modulations: Modulations::of(list),
            protocol: Protocol::Unstated,
        }
    }

    #[test]
    fn the_synchronisation_octets_are_the_bits_in_table_one() {
        // Ten bits each, which is a start bit, eight data bits and a stop bit,
        // so both are ordinary framed octets. CI is `0000000001` and CM and JM
        // are `0000001111`, with b0 first.
        assert_eq!(SYNC_CI, 0b0000_0000);
        assert_eq!(SYNC_MENU, 0b1110_0000);
    }

    #[test]
    fn a_data_call_says_so_in_the_call_function_octet() {
        // Table 3: the tag is 1000 in b0..b3, b4 marks it a category octet,
        // and 011 in b5..b7 is "data (unspecified application)".
        let octets = data_menu(&[]).octets();
        let callf0 = octets[0];
        assert_eq!(tag_of(callf0), tag::CALL_FUNCTION);
        assert!(!bit(callf0, 4), "not marked as a category octet");
        assert_eq!(
            [bit(callf0, 5), bit(callf0, 6), bit(callf0, 7)],
            [false, true, true]
        );
    }

    #[test]
    fn every_call_function_survives_the_round_trip() {
        for f in [
            CallFunction::MultimediaTerminal,
            CallFunction::Textphone,
            CallFunction::Videotext,
            CallFunction::TransmitFax,
            CallFunction::ReceiveFax,
            CallFunction::Data,
        ] {
            let menu =
                Menu { function: f, modulations: Modulations::NONE, protocol: Protocol::Unstated };
            assert_eq!(Menu::parse(&menu.octets()).unwrap().function, f);
        }
    }

    #[test]
    fn the_modulation_octets_carry_their_tag_and_their_extension_code() {
        let octets = data_menu(&[Modulation::V32bis]).octets();
        assert_eq!(tag_of(octets[1]), tag::MODULATION, "modn0 tag");
        assert!(!bit(octets[1], 4), "modn0 not marked a category octet");
        assert!(!bit(octets[1], 5), "claimed a PCM category that is not there");
        for extension in &octets[2..] {
            assert!(
                is_extension(*extension),
                "{extension:08b} is not marked as an extension octet"
            );
        }
    }

    #[test]
    fn every_modulation_survives_the_round_trip() {
        // One at a time, so a bit written into the wrong octet or the wrong
        // position cannot be hidden by a neighbour.
        for m in Modulation::ALL {
            let menu = data_menu(&[m]);
            let back = Menu::parse(&menu.octets()).expect("parsed");
            assert_eq!(
                back.modulations,
                Modulations::of(&[m]),
                "{} came back as {:?}",
                m.name(),
                back.modulations.iter().map(Modulation::name).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn a_full_menu_survives_the_round_trip() {
        let menu = data_menu(&Modulation::ALL);
        assert_eq!(Menu::parse(&menu.octets()).unwrap(), menu);
    }

    #[test]
    fn no_modulation_shares_a_bit_with_another() {
        let mut seen = std::collections::HashSet::new();
        for m in Modulation::ALL {
            assert!(seen.insert(m.place()), "{} shares a bit", m.name());
        }
    }

    #[test]
    fn no_modulation_sits_where_the_structure_does() {
        // b0..b4 of modn0 are the tag and the category marker, b5 says whether
        // a PCM category follows, and b3..b5 of the extension octets are the
        // code that marks them as extensions. A modulation bit landing on any
        // of those would be read as something else entirely.
        for m in Modulation::ALL {
            let (which, b) = m.place();
            if which == 0 {
                assert!(b >= 6, "{} sits in modn0's structure", m.name());
            } else {
                assert!(
                    !(3..=5).contains(&b),
                    "{} sits in an extension octet's marker",
                    m.name()
                );
            }
        }
    }

    #[test]
    fn the_joint_menu_is_what_both_ends_have() {
        // 7.4: JM indicates "all modulation modes that are both indicated in
        // CM and available in the answer DCE".
        let theirs = data_menu(&[Modulation::V34Duplex, Modulation::V32bis, Modulation::V21]);
        let ours = Modulations::of(&[Modulation::V32bis, Modulation::V22bis]);
        let jm = theirs.joint(ours, Protocol::Unstated);
        assert_eq!(jm.modulations, Modulations::of(&[Modulation::V32bis]));
        assert_eq!(jm.function, CallFunction::Data, "the call function is carried over");
    }

    #[test]
    fn the_protocol_tag_reads_the_same_way_round_as_the_others() {
        // Table 2 prints b0 first and b0 is the least significant bit, so
        // every tag is written backwards from the number it is. The protocol
        // tag is "0101" where the modulation tag is "1010", which makes the
        // two constants mirror images -- and if either were read straight off
        // the table, they would come out as each other.
        assert_eq!(tag::PROTOCOL, 0b1010);
        assert_eq!(tag::MODULATION, 0b0101);
        assert_eq!(tag::PROTOCOL.reverse_bits() >> 4, tag::MODULATION);
    }

    #[test]
    fn the_lapm_octet_is_the_one_table_6_describes() {
        // Table 6: tag "0101" in b0-b3, zero in b4 for a category octet, and
        // "100" in b5-b7 for LAPM according to ITU-T V.42.
        let menu = Menu {
            function: CallFunction::Data,
            modulations: Modulations::of(&[Modulation::V22bis]),
            protocol: Protocol::Lapm,
        };
        let prot0 = *menu.octets().last().unwrap();
        assert_eq!(tag_of(prot0), tag::PROTOCOL);
        assert!(!bit(prot0, 4), "a category octet, not an extension");
        assert_eq!((bit(prot0, 5), bit(prot0, 6), bit(prot0, 7)), (true, false, false));
        assert_eq!(prot0, 0b0010_1010);
    }

    #[test]
    fn a_protocol_octet_is_not_read_as_a_modulation_extension() {
        // The hazard the parser has to survive: the modulation category
        // swallows the octets after it that look like its own extensions, and
        // an extension is told apart by "010" in b3..b5. The protocol octet
        // has a one in b3, so it is not one -- but only by that one bit, and
        // if it were lost the modulation bits would be read out of a protocol
        // octet and the call would be offered modes nobody has.
        let menu = Menu {
            function: CallFunction::Data,
            modulations: Modulations::of(&[Modulation::V21]),
            protocol: Protocol::Lapm,
        };
        let octets = menu.octets();
        let prot0 = *octets.last().unwrap();
        assert!(!is_extension(prot0), "b3 is what keeps these apart");
        let back = Menu::parse(&octets).unwrap();
        assert_eq!(back, menu, "everything survives, in both categories");
    }

    #[test]
    fn a_menu_that_says_nothing_about_protocol_says_nothing() {
        // Table 6's note: "absence of this octet does not preclude alternative
        // means of protocol negotiation". Silence is not a refusal, and the
        // V.42 detection phase is exactly the alternative it means.
        let menu = data_menu(&[Modulation::V32bis]);
        assert_eq!(menu.protocol, Protocol::Unstated);
        assert!(!menu.lapm());
        assert_eq!(Menu::parse(&menu.octets()).unwrap().protocol, Protocol::Unstated);
    }

    #[test]
    fn a_protocol_named_in_an_extension_octet_is_not_lapm() {
        // Table 6 gives "111" to a protocol named in an extension octet. This
        // modem does not read the extension, and the one thing it must not do
        // is take a code it cannot read for the one it can.
        let prot0 = octet([false, true, false, true, false, true, true, true]);
        let mut octets = data_menu(&[Modulation::V21]).octets();
        octets.push(prot0);
        assert_eq!(Menu::parse(&octets).unwrap().protocol, Protocol::Extended);
        assert!(!Menu::parse(&octets).unwrap().lapm());
    }

    #[test]
    fn a_joint_menu_names_lapm_only_when_both_ends_did() {
        // 7.4: "if the LAPM protocol code is indicated in CM, the protocol
        // octet may be included in JM in order to complete the negotiation".
        let asked = Menu {
            function: CallFunction::Data,
            modulations: Modulations::of(&[Modulation::V32bis]),
            protocol: Protocol::Lapm,
        };
        let ours = Modulations::of(&[Modulation::V32bis]);
        assert!(asked.joint(ours, Protocol::Lapm).lapm(), "both said it");
        assert!(!asked.joint(ours, Protocol::Unstated).lapm(), "the answerer did not");

        let silent = data_menu(&[Modulation::V32bis]);
        assert!(
            !silent.joint(ours, Protocol::Lapm).lapm(),
            "the call never asked, so there is nothing to complete"
        );
    }

    #[test]
    fn nothing_in_common_is_said_rather_than_left_unsaid() {
        // 7.4: with no modes in common the JM carries the same number of
        // modulation octets "and show zeros for all modulation modes". An
        // answering modem that simply stopped talking would be informing
        // nobody of anything.
        let theirs = data_menu(&[Modulation::V34Duplex]);
        let jm = theirs.joint(Modulations::of(&[Modulation::V21]), Protocol::Unstated);
        assert!(jm.modulations.is_empty());
        assert_eq!(jm.chosen(), None);
        assert_eq!(jm.octets().len(), theirs.octets().len(), "fewer octets than CM");
    }

    #[test]
    fn the_fastest_thing_both_ends_have_is_the_one_chosen() {
        // 7.4 settles it by item number, and Table 4 numbers them from V.34
        // downwards, so the lowest item number is the fastest modulation.
        let menu = data_menu(&[Modulation::V21, Modulation::V32bis, Modulation::V22bis]);
        assert_eq!(menu.chosen(), Some(Modulation::V32bis));

        let menu = data_menu(&[Modulation::V21, Modulation::V22bis]);
        assert_eq!(menu.chosen(), Some(Modulation::V22bis));

        let menu = data_menu(&[Modulation::V21]);
        assert_eq!(menu.chosen(), Some(Modulation::V21));
    }

    #[test]
    fn a_category_nobody_here_understands_is_stepped_over() {
        // Clause 5: a receiver "shall ignore all bits, codes and octets
        // reserved for such future definition", and clause 10 says the
        // Recommendation is meant to be extended. A menu carrying a category
        // invented after this was written is one to read the rest of.
        let mut octets = data_menu(&[Modulation::V22bis]).octets();
        // A non-standard facilities category, tag 1111, which this does not
        // implement and must not choke on.
        octets.insert(1, octet([true, true, true, true, false, false, false, false]));
        let back = Menu::parse(&octets).expect("a menu with an unknown category");
        assert_eq!(back.function, CallFunction::Data);
        assert_eq!(back.modulations, Modulations::of(&[Modulation::V22bis]));
    }

    #[test]
    fn a_menu_that_stops_early_is_still_read() {
        // A far end may send fewer modulation octets than three. What it did
        // send still means what it says.
        let full = data_menu(&[Modulation::V34Duplex, Modulation::V32bis]).octets();
        let short = &full[..2];
        let back = Menu::parse(short).expect("two octets");
        assert!(back.modulations.contains(Modulation::V34Duplex));
        assert!(!back.modulations.contains(Modulation::V32bis), "read past the end");
    }

    #[test]
    fn a_ci_carries_the_call_function_and_nothing_else() {
        // 7.1: "a CI sequence consists of 10 ONEs followed by 10
        // synchronization bits and the call function octet".
        let menu = data_menu(&Modulation::ALL);
        let ci = sequence(Signal::Ci, &menu);
        assert_eq!(ci.len(), 2, "a CI is the sync and one octet");
        assert_eq!(ci[0], SYNC_CI);
        assert_eq!(tag_of(ci[1]), tag::CALL_FUNCTION);
    }

    #[test]
    fn a_call_menu_is_the_sync_and_every_category() {
        let menu = data_menu(&[Modulation::V32bis]);
        let cm = sequence(Signal::Cm, &menu);
        assert_eq!(cm[0], SYNC_MENU);
        assert_eq!(&cm[1..], &menu.octets()[..]);
    }

    #[test]
    fn a_decoder_reads_back_what_a_sequence_wrote() {
        // Sent repeatedly, as 7.3 requires, and the repeat is what says the
        // first one has ended.
        let menu = data_menu(&[Modulation::V32bis, Modulation::V22bis]);
        let mut decoder = Decoder::new();
        let mut heard = None;
        for _ in 0..3 {
            for octet in sequence(Signal::Cm, &menu) {
                if let Some(h) = decoder.feed(octet) {
                    heard = Some(h);
                }
            }
        }
        assert_eq!(heard, Some(Heard::Cm(menu)));
    }

    #[test]
    fn a_decoder_finds_the_terminator() {
        // 3.5: three octets of all zeros, which ends CM once JM has been seen.
        let mut decoder = Decoder::new();
        let mut heard = None;
        for octet in CJ {
            if let Some(h) = decoder.feed(octet) {
                heard = Some(h);
            }
        }
        assert_eq!(heard, Some(Heard::Cj));
    }

    #[test]
    fn a_menu_is_read_to_its_end_and_not_to_a_length() {
        // Clause 5: "a receiver shall ignore all bits, codes and octets
        // reserved for such future definition". A decoder that stops counting
        // at the length of the menus it happens to build cannot do that, and
        // this one used to stop at four -- so a fifth category was not ignored
        // but lost, along with anything a real modem might put after it. V.8
        // describes several: PSTN access, PCM modem availability, non-standard
        // facilities. Here the trailing octet is the PSTN access category,
        // which this modem does not read and must still read past.
        let mut menu = data_menu(&[Modulation::V32bis, Modulation::V22bis]);
        menu.protocol = Protocol::Lapm;
        let access0 = octet([true, false, true, true, false, false, false, false]);
        assert_eq!(tag_of(access0), 0b1101, "the PSTN access tag of Table 2");

        let mut decoder = Decoder::new();
        let mut heard = None;
        for _ in 0..3 {
            let mut cm = sequence(Signal::Cm, &menu);
            cm.push(access0);
            for o in cm {
                if let Some(h) = decoder.feed(o) {
                    heard = Some(h);
                }
            }
        }
        assert_eq!(heard, Some(Heard::Cm(menu)), "a category past the end costs nothing");
    }

    #[test]
    fn rubbish_before_a_sequence_does_not_stop_it_being_read() {
        // The line before a menu is not clean. Whatever the framer made of the
        // answer tone dying away arrives first, and the sync is what says the
        // menu has started.
        let menu = data_menu(&[Modulation::V22bis]);
        let mut decoder = Decoder::new();
        let mut heard = None;
        for octet in [0x5a, 0xff, 0x13] {
            decoder.feed(octet);
        }
        for _ in 0..2 {
            for octet in sequence(Signal::Cm, &menu) {
                if let Some(h) = decoder.feed(octet) {
                    heard = Some(h);
                }
            }
        }
        assert_eq!(heard, Some(Heard::Cm(menu)));
    }
}
