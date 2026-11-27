//! Colour and greyscale pages in whole fax calls, through the modem crate.
//!
//! The answering end offers JPEG and full colour mode in its DIS (Table
//! 2/T.30 bits 68 and 69, and T.30 Annex E). A calling end with a picture to
//! send commands it in its DCS, and the page goes as T.4 Annex E has it: one
//! baseline JPEG data stream of CIELAB components, in error correction mode's
//! frames, each octet least significant bit first. The answering end decodes
//! it as the frames come in, and keeps both the picture and the stream.
//!
//! Both ends are ours. The calling end's part is the least that sends such a
//! page, there so that the answering end's part can be tested.

use fax::call::Phase;
use fax::coding::Coding;
use fax::jpeg;
use fax::page::{Page, Resolution};
use fax::picture::{Picture, lab_of_rgb};
use fax::t30::{self, Frame, Modulation};
use modem::FaxCall;

const FS: f64 = 16_000.0;

const WITH_V17: [Modulation; 3] = [Modulation::V27ter, Modulation::V29, Modulation::V17];

/// A page with a ramp of grey, bands of colour, and strokes like type: the
/// width of the paper, and `lines` long.
fn a_picture(lines: usize, components: usize) -> Picture {
    let width = fax::page::WIDTH;
    let mut samples = Vec::with_capacity(width * lines * components);
    for y in 0..lines {
        for x in 0..width {
            let ramp = (x * 255 / (width - 1)) as u8;
            let rgb = if y % 32 < 6 && (x / 9) % 5 < 3 {
                [20, 20, 30]
            } else {
                match x * 6 / width {
                    0 => [ramp.saturating_mul(6), 60, 60],
                    1 => [250, 250, 244],
                    2 => [40, 130 + (y % 64) as u8, 70],
                    3 => [ramp, ramp, ramp],
                    4 => [60, 90, 200],
                    _ => [245, 215, 70],
                }
            };
            let lab = lab_of_rgb(rgb);
            samples.extend_from_slice(&lab[..components]);
        }
    }
    Picture { width, height: lines, components, samples, pels_per_inch: 200, ..Picture::default() }
}

/// The page of ink and paper that goes if the far end reads no picture.
fn a_page(lines: usize) -> Page {
    let width = fax::page::WIDTH;
    Page {
        lines: (0..lines).map(|y| (0..width).map(|x| y % 32 < 6 && (x / 9) % 5 < 3).collect()).collect(),
        resolution: Resolution::Fine,
    }
}

/// What a call came to.
#[derive(Debug, Default)]
struct Outcome {
    page: Option<Page>,
    picture: Option<Picture>,
    /// The parameter field of the DIS the caller heard, and of the DCS the
    /// answerer heard.
    dis: Vec<u8>,
    dcs: Vec<u8>,
    /// Every frame the answering end heard.
    heard: Vec<Frame>,
    /// The answering end's line count each time it changed while the page
    /// was still arriving, and whether those lines were there as a picture.
    seen: Vec<usize>,
    seen_in_tone: bool,
}

/// Two fax calls against each other, with `noise` applied to what reaches the
/// answering end.
fn call(
    caller: &mut FaxCall,
    answerer: &mut FaxCall,
    seconds: f64,
    noise: &mut dyn FnMut(f64, Phase, Option<f64>) -> f64,
) -> Outcome {
    let (mut to_caller, mut to_answerer) = (0.0, 0.0);
    let mut outcome = Outcome::default();
    for _ in 0..(seconds * FS) as usize {
        let a = caller.step(to_caller);
        let b = answerer.step(to_answerer);
        to_caller = b;
        to_answerer = noise(a, caller.phase(), caller.progress());
        for message in caller.take_heard() {
            if message.frame == Frame::Dis {
                outcome.dis = message.fif;
            }
        }
        for message in answerer.take_heard() {
            if message.frame == Frame::Dcs {
                outcome.dcs.clone_from(&message.fif);
            }
            outcome.heard.push(message.frame);
        }
        if answerer.phase() == Phase::Receiving && outcome.page.is_none() {
            let lines = answerer.lines().len();
            if outcome.seen.last() != Some(&lines) {
                outcome.seen.push(lines);
                if lines > 0 {
                    let part = answerer.arriving_picture(0, lines);
                    outcome.seen_in_tone = part.is_some_and(|p| p.height == lines);
                }
            }
        }
        if outcome.page.is_none() {
            outcome.page = answerer.take_received().map(|(_, page)| page);
        }
        if outcome.picture.is_none() {
            outcome.picture = answerer.take_received_picture().map(|(_, picture)| picture);
        }
        if caller.phase().is_over() && answerer.phase().is_over() {
            break;
        }
    }
    outcome
}

fn clean(sample: f64, _: Phase, _: Option<f64>) -> f64 {
    sample
}

/// The stream a picture is sent as, and what that stream decodes to.
fn sent(picture: &Picture) -> (Vec<u8>, Picture) {
    let stream = jpeg::encode(picture, jpeg::Options::default());
    let mut decoder = jpeg::Decoder::new(fax::page::WIDTH);
    decoder.feed(&stream);
    (stream, decoder.picture().expect("our own stream does not decode"))
}

fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| f64::from(x.abs_diff(*y))).sum::<f64>() / a.len().max(1) as f64
}

fn ended_well(caller: &FaxCall, answerer: &FaxCall) {
    assert_eq!(caller.phase(), Phase::Done, "the caller ended at {} ({:?})", caller.phase().name(), caller.trouble());
    assert_eq!(answerer.phase(), Phase::Done, "the answerer ended at {} ({:?})", answerer.phase().name(), answerer.trouble());
    assert!(caller.error_correction() && answerer.error_correction());
}

#[test]
fn a_colour_page_arrives_as_the_picture_that_was_sent() {
    let picture = a_picture(96, 3);
    let (stream, decoded) = sent(&picture);
    let mut caller = FaxCall::originate(FS, "61399990000", Some(a_page(96)))
        .offering(&WITH_V17)
        .with_picture(Picture { jpeg: stream.clone(), ..picture.clone() });
    let mut answerer = FaxCall::answer(FS, "61388880000").offering(&WITH_V17);
    let got = call(&mut caller, &mut answerer, 90.0, &mut clean);
    ended_well(&caller, &answerer);

    // The DIS offered it: bits 68 and 69, beside 15 and 27 (Note 25).
    for bit in [15, 27, 68, 69] {
        assert!(t30::extended_bit(&got.dis, bit), "DIS bit {bit} is clear: {:02x?}", got.dis);
    }
    assert!(t30::capabilities(&got.dis).jpeg && t30::capabilities(&got.dis).full_colour);
    // And the DCS commanded it.
    assert!(t30::commands_jpeg(&got.dcs) && t30::commands_full_colour(&got.dcs), "{:02x?}", got.dcs);
    assert_eq!(caller.coding(), Coding::Jpeg);
    assert_eq!(answerer.coding(), Coding::Jpeg);
    assert!(answerer.full_colour());

    let arrived = got.picture.expect("no picture arrived");
    assert_eq!((arrived.width, arrived.height, arrived.components), (fax::page::WIDTH, 96, 3));
    assert_eq!(arrived.pels_per_inch, 200);
    // Error correction mode loses nothing: the stream is the stream that
    // went, and the picture is what that stream decodes to...
    assert_eq!(arrived.jpeg, stream, "the stream came out different");
    assert!(arrived.samples == decoded.samples, "the picture is not what the stream decodes to");
    // ...which is the picture that was sent, within what JPEG costs.
    let mean = mean_difference(&arrived.samples, &picture.samples);
    assert!(mean < 3.0, "the picture differs from the one sent by {mean} a sample on average");
    // And as colour on a screen, too.
    let mean = mean_difference(&arrived.rgb(), &picture.rgb());
    assert!(mean < 6.0, "as sRGB it differs by {mean} a sample on average");

    // The page of ink and paper comes with it, the same length, for
    // everything that only knows that.
    let page = got.page.expect("no page arrived");
    assert_eq!(page.lines.len(), 96);
    assert_eq!(page.resolution, Resolution::Fine);
    // Drawn as it came, and in tone, not all at the end.
    let partway = got.seen.iter().filter(|&&n| n > 0 && n < 96).count();
    assert!(partway >= 3, "seen at {partway} heights on the way ({:?})", got.seen);
    assert!(got.seen_in_tone, "the lines arriving were not there as a picture");
}

#[test]
fn a_greyscale_page_arrives_as_lightness_alone() {
    let picture = a_picture(64, 1);
    let (stream, decoded) = sent(&picture);
    let mut caller = FaxCall::originate(FS, "61399990000", Some(a_page(64))).with_picture(picture.clone());
    let mut answerer = FaxCall::answer(FS, "61388880000");
    let got = call(&mut caller, &mut answerer, 90.0, &mut clean);
    ended_well(&caller, &answerer);
    // Bit 68 without 69: "only the lightness component" (Note 35).
    assert!(t30::commands_jpeg(&got.dcs) && !t30::commands_full_colour(&got.dcs), "{:02x?}", got.dcs);
    assert!(!answerer.full_colour());
    let arrived = got.picture.expect("no picture arrived");
    assert_eq!((arrived.width, arrived.height, arrived.components), (fax::page::WIDTH, 64, 1));
    assert_eq!(arrived.jpeg, stream);
    assert!(arrived.samples == decoded.samples);
    assert!(mean_difference(&arrived.samples, &picture.samples) < 3.0);
}

#[test]
fn colour_goes_only_where_the_answering_end_offers_it_and_error_correction_is_on() {
    // Off at the answering end, or with no error correction to carry it, the
    // DIS does not have bit 68 and the page of ink and paper goes instead.
    for (colour, error_correction, want) in
        [(false, true, Coding::Jbig), (true, false, Coding::ModifiedRead)]
    {
        let page = a_page(48);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone())).with_picture(a_picture(48, 3));
        let mut answerer = FaxCall::answer(FS, "61388880000")
            .with_colour(colour)
            .with_error_correction(error_correction);
        let got = call(&mut caller, &mut answerer, 90.0, &mut clean);
        let case = format!("colour {colour}, error correction {error_correction}");
        assert!(!t30::capabilities(&got.dis).jpeg, "{case}: {:02x?}", got.dis);
        assert_eq!(caller.coding(), want, "{case}");
        assert_eq!(answerer.coding(), want, "{case}");
        assert!(got.picture.is_none(), "{case}: a picture arrived");
        assert!(got.page.expect("no page arrived").lines == page.lines, "{case}");
    }
}

#[test]
fn a_burst_of_noise_costs_a_colour_page_frames_and_not_the_picture() {
    // A wrong bit in a Huffman-coded scan puts everything after it wrong, to
    // the next restart marker or the end of the page. Under error correction
    // mode the frames the burst lands on are asked for and sent again.
    let picture = a_picture(160, 3);
    let (stream, decoded) = sent(&picture);
    let mut caller = FaxCall::originate(FS, "61399990000", Some(a_page(160))).with_picture(picture);
    let mut answerer = FaxCall::answer(FS, "61388880000");
    let mut seed = 0x1234_5678u32;
    let mut started: Option<usize> = None;
    let mut sample = 0usize;
    let mut burst = |a: f64, phase: Phase, progress: Option<f64>| {
        sample += 1;
        if started.is_none() && phase == Phase::Sending && progress.is_some_and(|p| p > 0.3) {
            started = Some(sample);
        }
        match started {
            // A fifth of a second of noise as loud as the signal.
            Some(at) if sample - at < (0.2 * FS) as usize => {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                a + (f64::from(seed) / f64::from(u32::MAX) - 0.5)
            }
            _ => a,
        }
    };
    let got = call(&mut caller, &mut answerer, 180.0, &mut burst);
    ended_well(&caller, &answerer);
    assert!(got.heard.iter().filter(|f| **f == Frame::Pps).count() >= 2, "nothing was asked for again: {:?}", got.heard);
    let arrived = got.picture.expect("no picture arrived");
    assert_eq!(arrived.jpeg, stream, "the stream came out different");
    assert!(arrived.samples == decoded.samples);
}

#[test]
fn a_colour_page_of_several_blocks_arrives_whole() {
    // A block of error correction mode is 256 frames of 256 octets (T.4
    // A.3.6.2), and a colour page is several: each is confirmed on its own,
    // and the decoder carries on across the joins.
    let mut picture = a_picture(480, 3);
    // Grain in the lightness, so that the page is as big as a scan of paper.
    let mut seed = 0x9e37_79b9u32;
    for pel in picture.samples.chunks_exact_mut(3) {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        pel[0] = pel[0].saturating_add((seed >> 27) as u8).saturating_sub(16);
    }
    let (stream, decoded) = sent(&picture);
    let blocks = stream.len().div_ceil(256 * 256);
    assert!(blocks >= 3, "the page is only {} octets, {blocks} blocks", stream.len());
    let mut caller = FaxCall::originate(FS, "61399990000", Some(a_page(480)))
        .offering(&WITH_V17)
        .with_picture(picture);
    let mut answerer = FaxCall::answer(FS, "61388880000").offering(&WITH_V17);
    let got = call(&mut caller, &mut answerer, 400.0, &mut clean);
    ended_well(&caller, &answerer);
    assert_eq!(got.heard.iter().filter(|f| **f == Frame::Pps).count(), blocks, "{:?}", got.heard);
    let arrived = got.picture.expect("no picture arrived");
    assert_eq!(arrived.height, 480);
    assert_eq!(arrived.jpeg, stream, "the stream came out different");
    assert!(arrived.samples == decoded.samples);
}

#[test]
fn a_colour_page_goes_over_super_g3() {
    // V.34 half-duplex (T.30 Annex F): the same DIS bits and the same page,
    // on the control and primary channels.
    let picture = a_picture(96, 3);
    let (stream, _) = sent(&picture);
    let mut caller = FaxCall::originate(FS, "61399990000", Some(a_page(96)))
        .offering(&WITH_V17)
        .with_v34(true)
        .with_picture(picture);
    let mut answerer = FaxCall::answer(FS, "61388880000").offering(&WITH_V17).with_v34(true);
    // Thirty milliseconds each way: V.34's probing wants a line with some
    // length to it.
    let delay = (0.030 * FS) as usize;
    let mut line: std::collections::VecDeque<f64> = std::iter::repeat_n(0.0, delay).collect();
    let mut delayed = |a: f64, _: Phase, _: Option<f64>| {
        line.push_back(a);
        line.pop_front().unwrap_or(0.0)
    };
    let got = call(&mut caller, &mut answerer, 240.0, &mut delayed);
    ended_well(&caller, &answerer);
    assert!(t30::commands_jpeg(&got.dcs) && t30::commands_full_colour(&got.dcs), "{:02x?}", got.dcs);
    assert!(t30::bit(&got.dis, 6), "the DIS is not Annex F's: {:02x?}", got.dis);
    assert_eq!(got.picture.expect("no picture arrived").jpeg, stream);
}
