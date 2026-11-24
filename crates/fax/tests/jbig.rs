//! JBIG against the test data of T.82 clause 7, and round trips of pages a fax
//! actually sends.
//!
//! 7.2 is explicit about what proves an implementation: an encoder "must
//! generate exactly the byte counts shown", and a decoder must decode what
//! such an encoder makes and "exactly generate the artificial image". Both are
//! here, on the whole 1960 by 1951 image, which is why these live in an
//! integration test and not beside the code: they take seconds in a debug
//! build and are meant for the release run of the workspace.

use fax::jbig::{self, ATMOVE, ESC, Header, NEWLEN, Options, SDNORM, STUFF};

/// A BIE taken apart the way Table 29 counts it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    scd: usize,
    pscd: usize,
    sde: usize,
    bid: usize,
    bie: usize,
    /// Each ATMOVE: the stripe it comes in front of, yAT, τX and τY.
    moves: Vec<(usize, u32, u8, u8)>,
}

/// Walk a BIE: every byte of PSCD, every ESC STUFF that is one byte of SCD,
/// every stripe's ESC SDNORM, and the floating marker segments between them.
/// Written apart from the decoder, so that the counts do not rest on it.
fn count(bie: &[u8]) -> Counts {
    let mut counts = Counts {
        bie: bie.len(),
        bid: bie.len() - Header::LEN,
        ..Counts::default()
    };
    let mut stripes = 0;
    let mut pscd = 0;
    let mut i = Header::LEN;
    while i < bie.len() {
        if bie[i] != ESC {
            counts.scd += 1;
            pscd += 1;
            i += 1;
            continue;
        }
        match bie[i + 1] {
            STUFF => {
                counts.scd += 1;
                pscd += 2;
                i += 2;
            }
            SDNORM => {
                counts.pscd += pscd;
                counts.sde += pscd + 2;
                pscd = 0;
                stripes += 1;
                i += 2;
            }
            ATMOVE => {
                let at = u32::from_be_bytes(bie[i + 2..i + 6].try_into().unwrap());
                counts.moves.push((stripes, at, bie[i + 6], bie[i + 7]));
                i += 8;
            }
            NEWLEN => i += 6,
            other => panic!("ESC {other:#04x} at byte {i}"),
        }
    }
    assert_eq!(pscd, 0, "data after the last SDE");
    counts
}

fn decode(bie: &[u8], width: usize) -> jbig::Decoder {
    let mut decoder = jbig::Decoder::new(width);
    decoder.feed_bytes(bie);
    decoder
}

/// The three tests of 7.2.2, with Table 29's byte counts for each.
///
/// The first two code the whole image as one stripe with the AT pixel held
/// still (MX = 0), in the three-line template and the two-line one. The third
/// is the one T.85 is made of: stripes of 128, typical prediction on, and the
/// AT pixel free to move up to eight pels -- which Annex C moves once, to
/// τX = 8 at the top of stripe 9 (Table 28), and which is the eight bytes by
/// which that test's BID outgrows its SDEs.
#[test]
fn seven_two_two_gives_table_29_to_the_byte_and_decodes_to_the_image() {
    let image = jbig::artificial_image();
    let tests = [
        (Options { stripe: 1951, mx: 0, two_line: false, typical_prediction: false, variable_length: false },
         [316_094, 317_362, 317_364, 317_364, 317_384]),
        (Options { stripe: 1951, mx: 0, two_line: true, typical_prediction: false, variable_length: false },
         [315_887, 317_110, 317_112, 317_112, 317_132]),
        (Options { stripe: 128, mx: 8, two_line: false, typical_prediction: true, variable_length: false },
         [252_557, 253_593, 253_625, 253_633, 253_653]),
    ];
    for (options, [scd, pscd, sde, bid, bie]) in tests {
        let coded = jbig::encode(&image, options);
        let counts = count(&coded);
        assert_eq!(
            [counts.scd, counts.pscd, counts.sde, counts.bid, counts.bie],
            [scd, pscd, sde, bid, bie],
            "SCD, PSCD, SDE, BID and BIE for {options:?}"
        );
        if options.mx == 0 {
            assert!(counts.moves.is_empty(), "an ATMOVE with MX 0");
        } else {
            assert_eq!(counts.moves, [(9, 0, 8, 0)], "not Table 28's one move");
        }
        let decoder = decode(&coded, 1960);
        assert!(decoder.is_done(), "{options:?}: not the whole page");
        assert_eq!(decoder.damaged(), 0);
        assert!(decoder.lines() == image.as_slice(), "{options:?}: not the image");
    }
}

#[test]
fn seven_two_two_decodes_the_same_arriving_a_few_bits_at_a_time() {
    // The third test again, fed as a line would feed it: bits, least
    // significant first, in uneven handfuls, with the lines checked as they
    // come.
    let image = jbig::artificial_image();
    let bie = jbig::encode(&image, Options { stripe: 128, mx: 8, two_line: false, typical_prediction: true, variable_length: false });
    let bits: Vec<bool> = bie.iter().flat_map(|&b| (0..8).map(move |i| b >> i & 1 == 1)).collect();
    let mut decoder = jbig::Decoder::new(1960);
    let mut at = 0;
    let mut seen = Vec::new();
    for step in (1..40).cycle() {
        if at >= bits.len() {
            break;
        }
        let end = (at + step).min(bits.len());
        decoder.feed_bits(&bits[at..end]);
        at = end;
        let got = decoder.lines().len();
        if seen.last() != Some(&got) {
            seen.push(got);
        }
    }
    assert!(decoder.is_done());
    assert!(decoder.lines() == image.as_slice());
    // It kept up: lines arrived in more than one step a stripe.
    assert!(seen.len() > 1951 / 2, "only {} steps for 1951 lines", seen.len());
}

/// A page of the kind a fax carries: lines of type with a rule now and then,
/// and under them a picture in ordered-dither grey, whose eight-pel period is
/// what the AT pixel is for.
fn fax_page(rows: usize) -> Vec<Vec<bool>> {
    const BAYER: [[u8; 8]; 8] = [
        [0, 32, 8, 40, 2, 34, 10, 42],
        [48, 16, 56, 24, 50, 18, 58, 26],
        [12, 44, 4, 36, 14, 46, 6, 38],
        [60, 28, 52, 20, 62, 30, 54, 22],
        [3, 35, 11, 43, 1, 33, 9, 41],
        [51, 19, 59, 27, 49, 17, 57, 25],
        [15, 47, 7, 39, 13, 45, 5, 37],
        [63, 31, 55, 23, 61, 29, 53, 21],
    ];
    (0..rows)
        .map(|y| {
            (0..fax::page::WIDTH)
                .map(|x| {
                    if y >= rows / 5 {
                        let grey = ((x + y) * 64 / (fax::page::WIDTH + rows)) as u8;
                        return BAYER[y % 8][x % 8] < grey;
                    }
                    if y % 97 == 50 {
                        return (100..1600).contains(&x);
                    }
                    let row = y / 24;
                    let within = y % 24;
                    within < 17 && x > 80 && x < 1650 && {
                        let x = x + (within / 6 + row) % 3;
                        (x + row * 7) % 29 < 4 || (within == 8 && (x + row) % 29 < 20)
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
fn fax_pages_come_back_whatever_the_options() {
    let page = fax_page(300);
    for stripe in [128, 1, 37, 1000] {
        for two_line in [false, true] {
            for typical_prediction in [false, true] {
                for mx in [0, 8, 127] {
                    let options = Options { stripe, mx, two_line, typical_prediction, variable_length: false };
                    let bie = jbig::encode(&page, options);
                    let decoder = decode(&bie, fax::page::WIDTH);
                    assert!(decoder.is_done(), "{options:?}");
                    assert_eq!(decoder.damaged(), 0, "{options:?}");
                    assert!(decoder.lines() == page.as_slice(), "{options:?}");
                }
            }
        }
    }
}

#[test]
fn the_dither_moves_the_at_pixel_and_that_pays() {
    // Annex C finds the band's period, and the page is smaller for it.
    let page = fax_page(600);
    let still = jbig::encode(&page, Options { mx: 0, ..Options::FAX });
    let free = jbig::encode(&page, Options::FAX);
    let moves = count(&free).moves;
    assert!(!moves.is_empty(), "the AT pixel never moved");
    assert!(moves.iter().all(|&(_, at, tx, ty)| at == 0 && ty == 0 && (3..=8).contains(&tx)), "{moves:?}");
    assert!(free.len() < still.len(), "{} bytes moving against {} still", free.len(), still.len());
    assert!(decode(&free, fax::page::WIDTH).lines() == page.as_slice());
}

#[test]
fn a_page_comes_to_less_than_mmr() {
    // T.82 Intro. 1: 1.1 to 1.5 times MMR's compression on scanned type, and
    // a great deal more on dithered grey. This page has both.
    let page = fax_page(1143);
    let jbig = jbig::encode(&page, Options::FAX).len() * 8;
    let mmr = fax::mmr::encode(&page).len();
    assert!(jbig * 3 < mmr * 2, "JBIG {jbig} bits against MMR's {mmr}");
}

/// Where each SDE of a BIE ends: the index just past its ESC SDNORM.
fn stripe_ends(bie: &[u8]) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut i = Header::LEN;
    while i < bie.len() {
        if bie[i] == ESC {
            match bie[i + 1] {
                SDNORM => ends.push(i + 2),
                ATMOVE => i += 6,
                NEWLEN => i += 4,
                _ => {}
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    ends
}

fn with_height(bie: &[u8], height: u32, stripe: u32) -> Vec<u8> {
    let mut out = bie.to_vec();
    out[8..12].copy_from_slice(&height.to_be_bytes());
    out[12..16].copy_from_slice(&stripe.to_be_bytes());
    out[19] |= 0x20; // VLENGTH
    out
}

fn new_length(height: u32) -> Vec<u8> {
    let mut out = vec![ESC, NEWLEN];
    out.extend_from_slice(&height.to_be_bytes());
    out
}

#[test]
fn a_page_of_unknown_length_ends_where_the_newlen_says() {
    // T.85 Amendment 1, Appendix I: a page of 500 lines sent by a machine that
    // did not know it was 500 when it started.
    let page = fax_page(500);
    let options = Options { mx: 0, ..Options::FAX };
    let exact = jbig::encode(&page, options);
    let ends = stripe_ends(&exact);
    assert_eq!(ends.len(), 4);

    // Basic mode-1: YD = 0xffffffff, and the NEWLEN in front of the fourth
    // stripe -- which is what the encoder's own variable length does.
    let own = jbig::encode(&page, Options { variable_length: true, ..options });
    let mut spliced = with_height(&exact[..ends[2]], u32::MAX, 128);
    spliced.extend_from_slice(&new_length(500));
    spliced.extend_from_slice(&exact[ends[2]..]);
    assert_eq!(own, spliced, "not I.1's basic mode-1");

    // Basic mode-2: YD = 1024, the fourth stripe sent with only its 116 lines,
    // and then NEWLEN and a null stripe.
    let mut late = with_height(&exact, 1024, 128);
    late.extend_from_slice(&new_length(500));
    late.extend_from_slice(&[ESC, SDNORM]);

    // Option mode: one stripe for the page, YD and L0 both 0xffffffff, and the
    // same NEWLEN and null stripe after it.
    let whole = jbig::encode(&page, Options { stripe: 500, ..options });
    let mut one = with_height(&whole, u32::MAX, u32::MAX);
    one.extend_from_slice(&new_length(500));
    one.extend_from_slice(&[ESC, SDNORM]);

    for (name, bie) in [("basic mode-1", own), ("basic mode-2", late), ("option mode", one)] {
        // All at once, and a byte at a time, which sees the lines past 500
        // that mode-2 decodes and then takes back.
        let decoder = decode(&bie, fax::page::WIDTH);
        assert!(decoder.is_done(), "{name}");
        assert_eq!(decoder.damaged(), 0, "{name}");
        assert!(decoder.lines() == page.as_slice(), "{name}: {} lines", decoder.lines().len());
        let mut decoder = jbig::Decoder::new(fax::page::WIDTH);
        let mut most = 0;
        for &byte in &bie {
            decoder.feed_bytes(&[byte]);
            most = most.max(decoder.lines().len());
            let n = decoder.lines().len().min(500);
            assert!(decoder.lines()[..n] == page[..n], "{name}: a wrong line on the way");
        }
        assert!(decoder.lines() == page.as_slice(), "{name}, a byte at a time");
        assert!(most <= 512, "{name}: {most} lines at most");
        assert_eq!(decoder.header().map(|h| h.variable_length), Some(true));
    }
}

#[test]
fn a_newlen_that_would_lengthen_the_page_is_not_believed() {
    // "The new YD shall never be greater than the original" (6.2.6.2).
    let page = fax_page(200);
    let exact = jbig::encode(&page, Options { mx: 0, ..Options::FAX });
    let mut bie = with_height(&exact, 200, 128);
    bie.extend_from_slice(&new_length(300));
    let decoder = decode(&bie, fax::page::WIDTH);
    assert!(decoder.is_done());
    assert!(decoder.lines() == page.as_slice());
}
