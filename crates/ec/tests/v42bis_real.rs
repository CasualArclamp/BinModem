//! V.42bis against a far end that is not this program.
//!
//! A round trip through our own encoder and decoder proves they agree with
//! each other, which is not the question. Both of them once dropped the same
//! dictionary entry at the same transition and the round trip passed the whole
//! time; what caught it was a board.

use ec::v42bis::{self, Decoder, Params};

/// 1957 octets a real modem sent, taken off a call to dialup.world and its
/// gateway on to bbs.fozztexx.com, reassembled from the LAPM I-frames in
/// sequence. The far end negotiates nothing -- it answers no XID at all -- and
/// simply announces compressed mode in band, then crosses between compressed
/// and transparent about every hundred octets, which is what makes it worth
/// keeping: 7.8.1 a) and 7.8.2 b) are both exercised a dozen times over.
const WIRE: &[u8] = include_bytes!("vectors/dialup-world.v42bis");

/// What the board actually put on the screen.
const PLAIN: &[u8] = include_bytes!("vectors/dialup-world.txt");

/// The parameters the far end is decoded with: it stated none, so these are
/// what this end offered and therefore the most it could have read.
fn offered() -> Params {
    Params { n2: v42bis::OFFERED_N2, n7: v42bis::OFFERED_N7 }
}

#[test]
fn a_board_that_never_answered_xid_still_reads_back_exactly() {
    let mut decoder = Decoder::new(offered());
    let mut out = Vec::new();
    decoder.decode(WIRE, &mut out).expect("decode failed");
    assert_eq!(
        String::from_utf8_lossy(&out),
        String::from_utf8_lossy(PLAIN),
        "the far end's text came back changed"
    );
}

#[test]
fn the_words_that_used_to_come_apart() {
    let mut decoder = Decoder::new(offered());
    let mut out = Vec::new();
    decoder.decode(WIRE, &mut out).expect("decode failed");
    let text = String::from_utf8_lossy(&out).into_owned();

    // Each of these is one entry's worth of drift, and they arrive in the
    // order the dictionary fell behind: the first transitions cost nothing
    // visible and the later ones cost a letter, then a word, then the line.
    for phrase in [
        "Character set [ASCII]:",
        "Running on an IWill KK266Plus KT133A",
        "FozzTexx in Objective-C. The computer",
        "has no hard drive and boots using PXE",
        "You are caller #137 for 07 Sep 2026",
        "-- Press a key --",
    ] {
        assert!(text.contains(phrase), "missing {phrase:?} in:\n{text}");
    }
}

/// The dictionary is the same size whichever way the stream is handed over.
///
/// A modem does not receive 1957 octets at once: it receives an I-frame, and
/// then another one. Feeding the decoder a byte at a time is the harshest
/// version of that, and it must not change a single output octet -- the mode
/// transitions and the codeword boundaries fall wherever they fall.
#[test]
fn arriving_in_pieces_decodes_the_same() {
    let mut whole = Decoder::new(offered());
    let mut expected = Vec::new();
    whole.decode(WIRE, &mut expected).expect("decode failed");

    let mut piecemeal = Decoder::new(offered());
    let mut out = Vec::new();
    for octet in WIRE {
        piecemeal.decode(&[*octet], &mut out).expect("decode failed");
    }
    assert_eq!(out, expected);
}
