//! The two halves against each other.
//!
//! A compressor is only correct if the far end gets back what went in, so
//! almost everything here is a round trip. The interesting cases are the ones
//! where the two ends could fall a step out of line with each other: a
//! codeword sent before the decoder has built it, an extension that reaches
//! characters still being written, a dictionary that fills, and a mode that
//! changes.

use super::*;

fn round_trip_with(input: &[u8], params: Params) -> Vec<u8> {
    let mut enc = Encoder::new(params);
    let mut wire = Vec::new();
    enc.encode(input, &mut wire);
    enc.flush(&mut wire);

    let mut dec = Decoder::new(params);
    let mut out = Vec::new();
    dec.decode(&wire, &mut out).expect("the far end could not read it");
    out
}

fn round_trip(input: &[u8]) -> Vec<u8> {
    round_trip_with(input, Params::default())
}

fn compressed_size(input: &[u8]) -> usize {
    let mut enc = Encoder::new(Params::default());
    let mut wire = Vec::new();
    enc.encode(input, &mut wire);
    enc.flush(&mut wire);
    wire.len()
}

#[test]
fn short_text_round_trips() {
    let text = b"Welcome to the board.";
    assert_eq!(round_trip(text), text);
}

#[test]
fn every_byte_value_round_trips() {
    // Also the only place the ordinal size has to grow: 7.11.1 starts at
    // seven bits and steps up the first time a value above 127 goes.
    let all: Vec<u8> = (0..=255u8).collect();
    assert_eq!(round_trip(&all), all);
}

#[test]
fn a_single_character_round_trips() {
    assert_eq!(round_trip(b"x"), b"x");
    assert_eq!(round_trip(b""), b"");
}

/// The Recommendation's own example string, from Appendix II.1.
#[test]
fn the_documents_example_string_round_trips() {
    let text = b"ABCDEXABCDEYABCDE\xffAC";
    assert_eq!(round_trip(text), text);
}

/// And II.2's, which is the run that makes a decoder read characters it is
/// still writing.
#[test]
fn a_run_of_one_character_round_trips() {
    let text = b"CCCCCCCCCCX";
    assert_eq!(round_trip(text), text);
    // Longer, so the extension procedure has to carry a match well past what
    // the dictionary held.
    let long: Vec<u8> = std::iter::repeat_n(b'C', 4000).chain(*b"X").collect();
    assert_eq!(round_trip(&long), long);
}

#[test]
fn repetitive_data_round_trips_and_compresses() {
    let text: Vec<u8> = b"the same line over and over\r\n"
        .iter()
        .copied()
        .cycle()
        .take(20_000)
        .collect();
    assert_eq!(round_trip(&text), text);
    let size = compressed_size(&text);
    assert!(size * 20 < text.len(), "20000 octets became {size}");
}

#[test]
fn english_like_text_round_trips_and_compresses() {
    let text: Vec<u8> = b"It is a truth universally acknowledged, that a single man in \
        possession of a good fortune, must be in want of a wife. However little known the \
        feelings or views of such a man may be on his first entering a neighbourhood, this \
        truth is so well fixed in the minds of the surrounding families. "
        .iter()
        .copied()
        .cycle()
        .take(40_000)
        .collect();
    assert_eq!(round_trip(&text), text);
    let size = compressed_size(&text);
    assert!(size * 4 < text.len(), "40000 octets became {size}");
}

/// Data with nothing to find in it must still come back, and must not grow
/// without bound.
#[test]
fn incompressible_data_round_trips() {
    let mut x: u32 = 0x1234_5678;
    let noise: Vec<u8> = (0..20_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x & 0xff) as u8
        })
        .collect();
    assert_eq!(round_trip(&noise), noise);
}

/// Handed over the way a link hands it over: a few hundred octets at a time,
/// flushed each time, for long enough that the dictionary fills more than once.
#[test]
fn chunked_traffic_survives_a_full_dictionary() {
    let params = Params::of(512, 64);
    let mut enc = Encoder::new(params);
    let mut dec = Decoder::new(params);

    let mut plain = Vec::new();
    let mut x: u32 = 0x2545_f491;
    for round in 0..80 {
        for i in 0..20 {
            plain.extend_from_slice(format!("GET /page/{i} HTTP/1.1\r\n").as_bytes());
        }
        for _ in 0..200 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            plain.push((x & 0xff) as u8);
        }
        let _ = round;
    }

    let mut back = Vec::new();
    let mut at = 0;
    while at < plain.len() {
        let take = 1 + (x as usize % 700);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        let end = (at + take).min(plain.len());
        let mut wire = Vec::new();
        enc.encode(&plain[at..end], &mut wire);
        enc.flush(&mut wire);
        dec.decode(&wire, &mut back).expect("the far end could not read it");
        at = end;
    }
    if back != plain {
        let i = back.iter().zip(&plain).position(|(a, b)| a != b).unwrap_or(0);
        panic!(
            "came apart at octet {i} of {}: sent {:?}, got {:?}",
            plain.len(),
            &plain[i..(i + 24).min(plain.len())],
            &back[i..(i + 24).min(back.len())]
        );
    }
    assert_eq!(back.len(), plain.len());
}

/// 7.11.3 and 7.11.4: a full node-tree or a full history starts the dictionary
/// again, and the far end has to be told rather than left to work it out.
#[test]
fn a_dictionary_that_fills_starts_again_without_losing_anything() {
    // Small enough that both limits are reached many times over.
    let params = Params { n2: 256, n7: 32, n8: 512 };
    let text: Vec<u8> = b"abcdefghij"
        .iter()
        .copied()
        .cycle()
        .take(30_000)
        .collect();
    assert_eq!(round_trip_with(&text, params), text);
}

/// 7.11.2: the codeword size grows as the dictionary does, announced each
/// time, and the far end follows without being told how far.
#[test]
fn the_codeword_size_grows_and_the_decoder_follows() {
    // Enough distinct strings to climb several codeword sizes.
    let mut text = Vec::new();
    for i in 0..3000u32 {
        text.extend_from_slice(format!("{i:05}-").as_bytes());
    }
    assert_eq!(round_trip(&text), text);
}

/// Every parameter combination the negotiation can land on has to work, not
/// just the default one.
#[test]
fn the_negotiable_parameters_all_work() {
    let text: Vec<u8> = b"a longer phrase repeated so that strings are found and extended, "
        .iter()
        .copied()
        .cycle()
        .take(9_000)
        .collect();
    for n2 in [256u16, 512, 1024, 2048, 8192] {
        for n7 in [32u8, 46, 47, 78, 142, 255] {
            let params = Params::of(n2, n7);
            assert_eq!(
                round_trip_with(&text, params),
                text,
                "n2 {n2} n7 {n7}"
            );
        }
    }
}

/// A string that reaches the agreed maximum length cannot be extended past it
/// (6.3.1: "the total length of the string cannot exceed N7T").
#[test]
fn a_string_stops_at_the_agreed_maximum_length() {
    let params = Params::of(1024, 32);
    let text: Vec<u8> = std::iter::repeat_n(b'z', 5_000).collect();
    assert_eq!(round_trip_with(&text, params), text);
}

/// Data handed over one octet at a time, which is what a terminal does.
#[test]
fn compression_survives_being_handed_data_a_byte_at_a_time() {
    let params = Params::default();
    let mut enc = Encoder::new(params);
    let mut dec = Decoder::new(params);
    let text: Vec<u8> = b"login: cactus\r\npassword: \r\nWelcome back.\r\n"
        .iter()
        .copied()
        .cycle()
        .take(4_000)
        .collect();

    let mut back = Vec::new();
    for &byte in &text {
        let mut wire = Vec::new();
        enc.encode(&[byte], &mut wire);
        enc.flush(&mut wire);
        dec.decode(&wire, &mut back).expect("decode failed");
    }
    assert_eq!(back, text);
}

/// 6.5: transparent mode carries the characters untouched, and the dictionary
/// starts again on the way back in so both ends agree about what it holds.
#[test]
fn transparent_mode_carries_characters_and_restarts_the_dictionary() {
    let params = Params::default();
    let mut enc = Encoder::new(params);
    let mut dec = Decoder::new(params);
    let mut wire = Vec::new();
    let mut back = Vec::new();

    enc.encode(b"some ordinary text to begin with, ordinary text", &mut wire);
    enc.flush(&mut wire);
    dec.decode(&wire, &mut back).expect("decode failed");

    // Out to transparent, where what is sent is what arrives.
    wire.clear();
    enc.enter_transparent_now(&mut wire);
    assert_eq!(enc.mode(), Mode::Transparent);
    enc.encode(b"straight through", &mut wire);
    enc.flush(&mut wire);
    dec.decode(&wire, &mut back).expect("decode failed");
    assert_eq!(dec.mode(), Mode::Transparent);

    // And back in, which 6.5.2 makes both ends start the dictionary again.
    wire.clear();
    enc.enter_compressed(&mut wire);
    enc.encode(b"and compressed once more, compressed once more", &mut wire);
    enc.flush(&mut wire);
    dec.decode(&wire, &mut back).expect("decode failed");
    assert_eq!(dec.mode(), Mode::Compressed);

    assert_eq!(
        back,
        b"some ordinary text to begin with, ordinary textstraight throughand compressed once more, compressed once more"
            .to_vec()
    );
}

/// 7.14: the ESCAPE appearing in transparent data is announced with EID and
/// then moves on by 51, so the same octet does not mean the same thing twice.
#[test]
fn an_escape_in_transparent_data_is_announced_and_moves_on() {
    let params = Params::default();
    let mut enc = Encoder::new(params);
    let mut dec = Decoder::new(params);
    let mut wire = Vec::new();
    enc.enter_transparent_now(&mut wire);
    // The ESCAPE starts at zero, so a run of zeroes is the hard case: each
    // one has to be announced against a different value.
    let text = vec![0u8, 0, 51, 0, 102, 51, 7];
    enc.encode(&text, &mut wire);
    enc.flush(&mut wire);

    let mut back = Vec::new();
    dec.decode(&wire, &mut back).expect("decode failed");
    assert_eq!(back, text);
}

/// 7.14: the ESCAPE is assigned 0 "at initialization of the data compression
/// function" and moves on by 51 at each EID, and nothing else touches it --
/// not ECM, whose reinitialisation (6.5.2) is of the dictionary, whose state
/// 7.5.1 lists without it. Built from the clause rather than from this
/// encoder, because an encoder and decoder that are wrong the same way agree.
///
/// Transparent, a zero announced (so the ESCAPE is 51), back into compressed
/// mode with the ESCAPE of 51, out again, and then 51 is the ESCAPE still:
/// `33 01` is the octet 0x33, and a zero is only a zero.
#[test]
fn the_escape_is_not_set_back_by_entering_compressed_mode() {
    let params = Params::default();
    // ETM from a dictionary in its initial state, which is where the decoder
    // starts and where it is again just after an ECM.
    let etm = {
        let mut enc = Encoder::new(params);
        let mut wire = Vec::new();
        enc.enter_transparent_now(&mut wire);
        wire
    };
    let mut stream = etm.clone();
    stream.extend([0x00, command::EID]);
    stream.extend([0x33, command::ECM]);
    stream.extend(&etm);
    stream.extend([0x33, command::EID, 0x00, 0x62]);

    let mut dec = Decoder::new(params);
    let mut out = Vec::new();
    dec.decode(&stream, &mut out).expect("a stream built from the clause would not decode");
    assert_eq!(out, [0x00, 0x33, 0x00, 0x62]);

    // And this encoder writes the transparent stretches of that same stream.
    let mut enc = Encoder::new(params);
    let mut wire = Vec::new();
    enc.enter_transparent_now(&mut wire);
    let mark = wire.len();
    enc.encode(&[0x00], &mut wire);
    enc.enter_compressed(&mut wire);
    assert_eq!(wire[mark..], [0x00, command::EID, 0x33, command::ECM]);
    enc.enter_transparent_now(&mut wire);
    let mark = wire.len();
    enc.encode(&[0x33, 0x00, 0x62], &mut wire);
    enc.flush(&mut wire);
    assert_eq!(wire[mark..], [0x33, command::EID, 0x00, 0x62]);
}

/// 7.15: a codeword above C1 is a procedural error rather than something to
/// guess at, because the two dictionaries have come apart.
#[test]
fn a_codeword_nothing_has_created_is_refused() {
    let params = Params::default();
    let mut dec = Decoder::new(params);
    let mut out = Vec::new();
    // Prefix "1" then a six-bit codeword of 60, which is far above C1 of 4.
    let mut w = crate::bits::BitWriter::new();
    let mut wire = Vec::new();
    w.write(1, 1, &mut wire);
    w.write(60, 6, &mut wire);
    w.align(&mut wire);
    assert_eq!(
        dec.decode(&wire, &mut out),
        Err(Error::UnknownCodeword(60))
    );
}

/// The compression actually has to beat V.42bis on text, or there is no
/// reason for any of this.
#[test]
fn it_beats_v42bis_on_text() {
    let text: Vec<u8> = b"The quick brown fox jumps over the lazy dog. Pack my box with \
        five dozen liquor jugs. How vexingly quick daft zebras jump! "
        .iter()
        .copied()
        .cycle()
        .take(60_000)
        .collect();

    let v44 = compressed_size(&text);

    let mut enc = crate::v42bis::Encoder::new(crate::v42bis::Params::default());
    let mut old = Vec::new();
    enc.encode(&text, &mut old);
    enc.flush(&mut old);

    println!("  60000 octets: V.42bis {} , V.44 {v44}", old.len());
    assert!(
        v44 < old.len(),
        "V.44 made {v44} octets against V.42bis {}",
        old.len()
    );
}
