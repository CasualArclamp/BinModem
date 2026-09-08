//! Where a V.32 modem is allowed to stop sending one rate sequence and start
//! the next.
//!
//! 5.3.2: "In order to mark the end of transmission of any rate signal other
//! than R1 (Figure 4), the modem shall first complete the transmission of the
//! current 16-bit rate sequence, and then transmit one 16-bit sequence E."
//!
//! The far end has its framing locked to those sequences -- 5.3.1 has it
//! looking for "two consecutive identical 16-bit sequences each with bits
//! B0-3, B7, 11 and 15 conforming to Table 6" -- so an E that starts anywhere
//! else is not an E to it. It is two words that are neither, followed by
//! scrambled ones.
//!
//! Which is what this modem sent. R3 arrives whenever the line brings it and
//! never on a boundary, and the transmitter changed sequence where it was
//! asked to. On a call to a real modem the last R2 was cut short after twelve
//! of its sixteen bits and the E put there, so the far end read
//! 0101000100011111 at its own alignment -- B0-3 neither 0000 nor 1111 --
//! waited for an E that was never going to arrive where it was looking, and
//! stopped transmitting one round trip later. Every V.32 call did this, and
//! the connection that followed carried noise.
//!
//! A loopback never saw it: both ends cut the sequence short in the same place
//! and read each other perfectly.

use datapump::v32::startup::{end_signal, rate_signal_for};
use datapump::v32::{BAUD, Mode, Signal, Transmitter};

/// Ten samples to the symbol, so a symbol boundary is a whole number of
/// samples and the test can count symbols without inferring anything.
const FS: f64 = BAUD * 10.0;
const SAMPLES_PER_SYMBOL: usize = 10;
/// Sixteen bits at two to the symbol.
const SYMBOLS_PER_SEQUENCE: u64 = 8;

fn rate() -> Signal {
    Signal::Rate(rate_signal_for(4800))
}

fn end() -> Signal {
    Signal::Rate(end_signal(rate_signal_for(4800)))
}

fn symbol(tx: &mut Transmitter) {
    for _ in 0..SAMPLES_PER_SYMBOL {
        tx.next_sample();
    }
}

#[test]
fn an_e_asked_for_mid_sequence_waits_for_the_end_of_it() {
    for asked_at in 1..SYMBOLS_PER_SEQUENCE {
        let mut tx = Transmitter::new(Mode::Call, FS);
        tx.set_signal(rate());
        while tx.rate_symbols() < asked_at {
            symbol(&mut tx);
        }

        tx.set_signal(end());
        assert!(
            tx.rate_pending(),
            "asked at symbol {asked_at}, which is mid-sequence, so it waits"
        );
        assert_eq!(tx.signal(), rate(), "the sequence in progress carries on");

        // Step until the E actually starts, remembering how much of the
        // interrupted sequence went out.
        let mut last = tx.rate_symbols();
        while tx.signal() != end() {
            last = tx.rate_symbols();
            symbol(&mut tx);
            assert!(
                last < SYMBOLS_PER_SEQUENCE || tx.signal() == end(),
                "the sequence ran past its sixteen bits"
            );
        }
        assert_eq!(
            last, SYMBOLS_PER_SEQUENCE,
            "asked at symbol {asked_at}, the E began after {last} symbols of \
             the sequence it interrupted rather than after the whole {SYMBOLS_PER_SEQUENCE}"
        );
        assert_eq!(tx.rate_symbols(), 1, "one symbol of E has gone out");
        assert!(!tx.rate_pending(), "nothing is owed any more");
    }
}

#[test]
fn an_e_asked_for_on_a_boundary_starts_there() {
    let mut tx = Transmitter::new(Mode::Call, FS);
    tx.set_signal(rate());
    while tx.rate_symbols() < SYMBOLS_PER_SEQUENCE {
        symbol(&mut tx);
    }
    tx.set_signal(end());
    assert!(!tx.rate_pending(), "a whole sequence had just finished");
    assert_eq!(tx.signal(), end(), "so the E is taken at once");
}

/// The E gets one whole sequence of its own, not what is left of somebody
/// else's.
///
/// The start-up gives the E eight symbols. If that count ran from where the E
/// was asked for rather than from where it began, the E would be cut short by
/// however long it had waited -- the same fault again, at the other end of it.
#[test]
fn the_e_is_a_whole_sequence_however_late_it_starts() {
    let mut tx = Transmitter::new(Mode::Call, FS);
    tx.set_signal(rate());
    symbol(&mut tx);
    symbol(&mut tx);
    symbol(&mut tx);
    let interrupted_at = tx.rate_symbols();
    assert!(interrupted_at > 0 && interrupted_at < SYMBOLS_PER_SEQUENCE);
    tx.set_signal(end());

    // The start-up's condition for having sent a whole E.
    let sent_a_whole_e =
        |tx: &Transmitter| !tx.rate_pending() && tx.rate_symbols() >= SYMBOLS_PER_SEQUENCE;
    let mut symbols = 0;
    while !sent_a_whole_e(&tx) {
        symbol(&mut tx);
        symbols += 1;
        assert!(symbols < 64, "the E never finished");
    }
    // What was owed to the interrupted sequence, and then a whole E.
    let owed = SYMBOLS_PER_SEQUENCE - interrupted_at;
    assert_eq!(
        symbols,
        owed + SYMBOLS_PER_SEQUENCE,
        "interrupted after {interrupted_at} symbols, so {owed} were owed, then eight of E"
    );
    assert_eq!(tx.signal(), end());
}
