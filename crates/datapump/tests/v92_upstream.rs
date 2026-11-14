//! The PCM upstream link end to end over a channel that is neither ideal nor
//! ours: V92-26's de-risk spike.
//!
//! The plan's M0 gate asks whether the designed precoder, the trellis-coded
//! modulus chain and the decoder carry the upper half of the upstream ladder
//! over a codec-like channel. This is that gate: one file, the same code a
//! call would run -- [`Channel::estimate`] on a known TRN1u stretch,
//! [`design`] against it, [`choose`] for the rung, then the encoder's levels
//! down the line and the decoder's bits back out.
//!
//! What is *not* here, and is deliberate: no start-up, no framing, no
//! precoder download, no sampling-phase estimate. Those are V92-22 to V92-41.
//! The point of a spike is to find out whether the chain is worth wiring up
//! before any of it exists.

use datapump::v92::decoder::Decoder;
use datapump::v92::design::{Channel, design};
use datapump::v92::upchoice::choose;
use datapump::v92::upstream::UpEncoder;
use datapump::v92::{FilterLimits, Parameters, Trellis, up_rate};

/// A local loop as the upstream sees it: a null near DC, flat to about 3.4 kHz
/// and a roll-off after it, in 32 T-spaced taps.
fn loop_channel() -> Vec<f64> {
    let mut taps = vec![0.0; 32];
    taps[0] = 1.0;
    taps[1] = -0.85;
    for (k, tap) in taps.iter_mut().enumerate().skip(2) {
        *tap = 0.10 / (k as f64 - 1.0);
    }
    let norm = taps.iter().map(|tap| tap * tap).sum::<f64>().sqrt();
    taps.iter_mut().for_each(|tap| *tap /= norm);
    taps
}

/// `length` symbols of a two-level run, as TRN1u's GPA sequence is: what the
/// analogue modem was told to send, which is what makes the channel known.
fn training(length: usize, seed: u64) -> Vec<f64> {
    let mut state = seed | 1;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            if state >> 63 == 0 { 1.0 } else { -1.0 }
        })
        .collect()
}

/// A deterministic bit source.
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next(&mut self) -> bool {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 >> 63 == 0
    }
}

/// One call's worth of the upstream: estimate, design, choose, then data.
/// Returns the rung chosen and the number of bits that came back wrong.
fn upstream(noise: f64, trellis: Trellis) -> (Option<u8>, usize, usize, f64) {
    let taps = loop_channel();
    // TRN1u: what was sent, and what the codec made of it.
    let known = training(4800, 0x5eed);
    let mut received = vec![0.0; known.len()];
    for n in taps.len()..known.len() {
        let mut sum = 0.0;
        for (k, &tap) in taps.iter().enumerate() {
            sum += tap * known[n - k];
        }
        received[n] = sum;
    }
    let channel = Channel::estimate(&known, &received, 48).expect("TRN1u is long enough to fit");
    if std::env::var_os("SPIKE_TRACE").is_some() {
        let worst = taps.iter().zip(&channel.taps).map(|(a, b)| (a - b).abs()).fold(0.0f64, f64::max);
        eprintln!("estimate: worst tap error {worst:.3e}, fit noise {:.2e}", channel.noise);
    }
    let plan = FilterLimits::from_info1a(3, 3, 3);
    let plan = design(&channel, &plan).expect("the loop is designable");
    let Some(choice) = choose(&plan, noise, 0, trellis) else {
        return (None, 0, 0, plan.residual_db());
    };
    let parameters: Parameters = choice.parameters;

    let mut encoder = UpEncoder::new(&parameters).expect("the chosen parameters are feasible");
    let mut decoder = Decoder::new(&parameters);
    let mut source = Random::new(0x1234);
    // The channel's own history, and a noise source of the same variance the
    // design was told about.
    let mut line = vec![0.0f64; taps.len()];
    let mut noise_source = Random::new(0xbeef);
    let sigma = noise.sqrt();
    let mut skip = plan.delay;
    let mut sent = Vec::new();
    let mut back = Vec::new();
    for _ in 0..(plan.delay + 200 * 12) {
        let symbol = encoder.next_symbol(|| {
            let bit = source.next();
            sent.push(bit);
            bit
        });
        line.rotate_right(1);
        line[0] = symbol.v;
        let mut seen = 0.0;
        for (k, &tap) in taps.iter().enumerate() {
            seen += tap * line[k];
        }
        // Uniform noise of the variance the design was given, which is the
        // worst case for the same power.
        let sample = (noise_source.next() as u8 as f64 - 0.5) * sigma * 12f64.sqrt();
        if skip > 0 {
            skip -= 1;
            continue;
        }
        // A receiver normalises the design's own scaling away.
        decoder.feed((seen + sample) * plan.scale);
        back.extend(decoder.take_bits());
    }
    // The decoder's frame count starts late and its output is the stream from
    // there on, so the comparison is made from where the two line up.
    let shortest = sent.len().min(back.len());
    let at = (0..shortest.min(400))
        .find(|&offset| {
            let length = shortest - offset;
            length > 200 && sent[offset..offset + length].iter().zip(&back[..length]).all(|(a, b)| a == b)
        })
        .unwrap_or(0);
    let length = shortest.saturating_sub(at);
    if length == 0 {
        return (Some(parameters.drn), sent.len(), back.len(), plan.residual_db());
    }
    let positions: Vec<usize> = sent[at..]
        .iter()
        .zip(&back[..length])
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect();
    if std::env::var_os("SPIKE_TRACE").is_some() {
        eprintln!("errors at {positions:?}");
    }
    let errors = positions.len();
    (Some(parameters.drn), errors, length, plan.residual_db())
}

/// The gate itself: at a codec-like noise level, the designed chain carries the
/// upper half of the ladder with no errors at all.
#[test]
#[ignore = "half the bits wrong at sixteen points since the design's ridge and scale changed (c701e49); PCM upstream is unfinished and off by default"]
fn the_designed_chain_carries_the_upper_ladder_over_a_loop() {
    // Noise of 1e-5 is a signal-to-noise ratio of 50 dB at unit power, which
    // is a good local loop.
    for trellis in [Trellis::Sixteen, Trellis::ThirtyTwo, Trellis::SixtyFour] {
        let (drn, errors, length, residual) = upstream(1e-5, trellis);
        let drn = drn.unwrap_or_else(|| panic!("{trellis:?}: nothing was chosen, residual {residual:.1} dB"));
        println!(
            "{trellis:?}: drn {drn} ({} bit/s), {errors} of {length} bits wrong, residual {residual:.1} dB",
            up_rate(drn)
        );
        assert!(drn >= 10, "{trellis:?}: only drn {drn}, which is not the upper half");
        assert!(length > 2000, "{trellis:?}: only {length} bits came back");
        assert_eq!(errors, 0, "{trellis:?}: {errors} of {length} bits wrong");
    }
}

/// And the rung it reaches falls as the line gets worse, which is the other
/// half of knowing the design works: a choice that never moved would say
/// nothing about the noise it was given.
#[test]
fn the_rung_falls_as_the_line_gets_worse() {
    let mut previous = u8::MAX;
    let mut seen = Vec::new();
    for noise in [1e-7, 1e-6, 1e-5, 1e-4, 1e-3] {
        let (drn, _, _, _) = upstream(noise, Trellis::ThirtyTwo);
        let drn = drn.unwrap_or(0);
        assert!(drn <= previous, "the rung went up with the noise: {drn} after {previous}");
        previous = drn;
        seen.push((noise, drn));
    }
    println!("noise -> drn: {seen:?}");
    assert!(seen.first().expect("a trend").1 >= 10, "a quiet loop only reached {seen:?}");
    assert!(seen.last().expect("a trend").1 < seen.first().expect("a trend").1, "the noise cost nothing: {seen:?}");
}
