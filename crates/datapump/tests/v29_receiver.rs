//! The V.29 receiver against the lines a fax burst meets.
//!
//! One burst at a time from our own transmitter, through a line that does one
//! thing to it, into a receiver used only through its public interface --
//! `set_rate`, `feed`, `take_bits`, `carrier`, `restart` -- as `modem::faxcall`
//! uses it. What comes back is scored the way `docs/design/slow-modes/fax-qam.md`
//! 7 scores a page: a payload of 1200 octets cut into eighths, each eighth
//! either found intact in the bits or not, so that a slip in the middle reads
//! as the part it cost and not as total loss; and the bit error rate against
//! wherever the payload best lines up.
//!
//! Each proof is a table, printed, and then held to what the rebuilt receiver
//! does (`docs/design/slow-modes/fax-qam.md` 3 is what the one before it did):
//! `cargo test -p datapump --release --test v29_receiver -- --nocapture
//! --test-threads 1` shows them.
//!
//! Signal to noise is Es/N0, as V.32's acceptance measures it: white noise
//! whose power in a band as wide as the symbol rate is the signal's power
//! over the ratio, the signal measured over the burst.

use std::f64::consts::{PI, TAU};

use datapump::v29::{BAUD, CARRIER, Rate, Receiver, Transmitter};
use dsp::Resampler;

const FS: f64 = 16_000.0;

const RATES: [Rate; 3] = [Rate::R9600, Rate::R7200, Rate::R4800];

/// Octets in a burst's payload, and the parts it is scored in.
const PAYLOAD: usize = 1200;
const PARTS: usize = 8;

/// Samples of line after a burst: long enough for any carrier detector to let
/// go and any pipeline to empty.
const TAIL: usize = 4800;

// ---------------------------------------------------------------------------
// The line.

/// A seeded generator: xorshift64*, with Box-Muller.
#[derive(Debug, Clone)]
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1 }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn gaussian(&mut self) -> f64 {
        let u1 = self.unit().max(1e-18);
        let u2 = self.unit();
        (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos()
    }
}

/// A single-sideband frequency shift, as `v32_receiver.rs` has it: the
/// analytic signal by a Hilbert transformer, turned, and its real part.
#[derive(Debug)]
struct Shifter {
    taps: Vec<f64>,
    hist: Vec<f64>,
    pos: usize,
    centre: usize,
    phase: f64,
    step: f64,
}

impl Shifter {
    fn new(hz: f64) -> Self {
        const N: usize = 255;
        let m = (N - 1) / 2;
        let mut taps = vec![0.0; N];
        for (d, tap) in taps.iter_mut().enumerate() {
            let k = d as isize - m as isize;
            if k % 2 == 0 {
                continue;
            }
            let w = 0.54 - 0.46 * (TAU * d as f64 / (N - 1) as f64).cos();
            *tap = 2.0 / (PI * k as f64) * w;
        }
        Self { taps, hist: vec![0.0; N], pos: 0, centre: m, phase: 0.0, step: hz / FS }
    }

    fn process(&mut self, x: f64) -> f64 {
        let n = self.hist.len();
        self.pos = (self.pos + 1) % n;
        self.hist[self.pos] = x;
        let mut q = 0.0;
        for (d, &tap) in self.taps.iter().enumerate() {
            if tap != 0.0 {
                q += tap * self.hist[(self.pos + n - d) % n];
            }
        }
        let i = self.hist[(self.pos + n - self.centre) % n];
        let turn = self.phase * TAU;
        self.phase += self.step;
        self.phase -= self.phase.floor();
        i * turn.cos() - q * turn.sin()
    }
}

/// The carrier moved by `hz`, and the far clock `ppm` fast.
fn offset(signal: &[f64], hz: f64, ppm: f64) -> Vec<f64> {
    let mut shifted: Vec<f64> = if hz == 0.0 {
        signal.to_vec()
    } else {
        // The shifter delays by half its length; the lead-in it eats is
        // silence, and the tail it needs is put on.
        let mut shifter = Shifter::new(hz);
        signal.iter().chain(std::iter::repeat_n(&0.0, 127)).map(|&x| shifter.process(x)).skip(127).collect()
    };
    if ppm != 0.0 {
        let mut clock = Resampler::new(FS * (1.0 + ppm * 1e-6), FS);
        let mut out = Vec::with_capacity(shifted.len());
        for &x in &shifted {
            clock.process(x, &mut out);
        }
        shifted = out;
    }
    shifted
}

/// The standard deviation of white noise at `snr_db` of Es/N0 against a
/// signal of mean power `power`.
fn sigma(power: f64, snr_db: f64) -> f64 {
    ((FS / 2.0) / BAUD * power * 10f64.powf(-snr_db / 10.0)).sqrt()
}

fn add_noise(line: &mut [f64], sigma: f64, seed: u64) {
    if sigma > 0.0 {
        let mut rng = Rng::new(seed);
        for x in line {
            *x += sigma * rng.gaussian();
        }
    }
}

/// The mean power of a burst, over what is not its opening silence.
fn burst_power(burst: &[f64]) -> f64 {
    let from = (FS * 0.025) as usize;
    let part = &burst[from.min(burst.len())..];
    part.iter().map(|x| x * x).sum::<f64>() / part.len().max(1) as f64
}

// ---------------------------------------------------------------------------
// The far end, and the near end listening.

fn payload(seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed ^ 0x7a29);
    (0..PAYLOAD).map(|_| (rng.next_u64() >> 56) as u8).collect()
}

/// One whole burst from our transmitter: the synchronizing signal, the
/// payload, the turn-off.
fn burst(rate: Rate, data: &[u8]) -> Vec<f64> {
    let mut tx = Transmitter::new(FS);
    tx.start(rate);
    tx.push_bytes(data);
    let mut out = Vec::new();
    while tx.is_transmitting() {
        if tx.trained() && tx.pending_bits() == 0 {
            tx.stop();
        }
        out.push(tx.next_sample());
    }
    out
}

/// Samples from the start of a burst to the start of its segment 2: segment
/// 1's 48 symbols, and the shaping pulse's six to its peak.
fn to_segment_2() -> usize {
    ((48.0 + 6.0) * FS / BAUD).round() as usize
}

/// What a receiver made of a line.
#[derive(Debug, Default)]
struct Heard {
    bits: Vec<bool>,
    /// The sample each rising edge of the carrier came at.
    rises: Vec<usize>,
    /// Samples with the carrier on in the stretch the caller asked about.
    on_in: usize,
}

/// Feed a line to a receiver set for `rate`, restarting it at each sample
/// listed, and count the carrier over `watch`.
fn listen(rate: Rate, line: &[f64], restarts: &[usize], watch: std::ops::Range<usize>) -> Heard {
    let mut rx = Receiver::new(FS);
    rx.set_rate(rate);
    rx.restart();
    let mut heard = Heard::default();
    let mut was = false;
    for (n, &x) in line.iter().enumerate() {
        if restarts.contains(&n) {
            rx.restart();
        }
        rx.feed(x);
        heard.bits.extend(rx.take_bits());
        let on = rx.carrier();
        if on && !was {
            heard.rises.push(n);
        }
        if on && watch.contains(&n) {
            heard.on_in += 1;
        }
        was = on;
    }
    heard
}

fn bits_of(bytes: &[u8]) -> Vec<bool> {
    bytes.iter().flat_map(|&b| (0..8).rev().map(move |i| b >> i & 1 != 0)).collect()
}

/// How much of a payload came back.
#[derive(Debug, Clone, Copy)]
struct Score {
    parts: usize,
    ber: f64,
}

fn score(data: &[u8], bits: &[bool]) -> Score {
    let want = bits_of(data);
    let part = want.len() / PARTS;
    let found = |needle: &[bool]| bits.windows(needle.len()).any(|w| w == needle);
    let parts = (0..PARTS).filter(|&p| found(&want[p * part..(p + 1) * part])).count();
    // Where the payload lines up best, by its first 256 bits; everything
    // missing or after the end counts as wrong.
    let head = &want[..256];
    let mut best = (usize::MAX, 0usize);
    for at in 0..bits.len().saturating_sub(head.len()) {
        let wrong = head.iter().zip(&bits[at..]).filter(|(a, b)| a != b).count();
        if wrong < best.0 {
            best = (wrong, at);
            if wrong == 0 {
                break;
            }
        }
    }
    let ber = if best.0 > head.len() / 4 {
        0.5
    } else {
        let got = &bits[best.1..];
        let wrong = want.iter().enumerate().filter(|&(i, b)| got.get(i) != Some(b)).count();
        wrong as f64 / want.len() as f64
    };
    Score { parts, ber }
}

fn name(rate: Rate) -> &'static str {
    match rate {
        Rate::R9600 => "9600",
        Rate::R7200 => "7200",
        Rate::R4800 => "4800",
    }
}

/// A burst after `lead` samples of line, with its carrier `hz` off and its
/// clock `ppm` fast, and noise at `snr_db` on the whole line, scored.
fn one(rate: Rate, lead: usize, hz: f64, ppm: f64, snr_db: Option<f64>, seed: u64) -> (Score, Heard) {
    let data = payload(seed);
    let sent = burst(rate, &data);
    let power = burst_power(&sent);
    let mut line = vec![0.0; lead];
    line.extend(offset(&sent, hz, ppm));
    line.extend(std::iter::repeat_n(0.0, TAIL));
    if let Some(snr) = snr_db {
        add_noise(&mut line, sigma(power, snr), seed ^ 0xa11);
    }
    let heard = listen(rate, &line, &[], 0..lead);
    (score(&data, &heard.bits), heard)
}

fn cell(s: Score) -> String {
    format!("{}/8 {:.0e}", s.parts, s.ber)
}

// ---------------------------------------------------------------------------
// The proofs.

/// Every rate, with the burst arriving at each of eight sample offsets: a
/// clean line, so that nothing but where the receiver's clock falls against
/// the far end's is different. fax-qam 3.3: 4800's A and B tie.
#[test]
fn every_rate_at_eight_arrival_phases() {
    let mut failed = Vec::new();
    for rate in RATES {
        let mut row = format!("{:>5}:", name(rate));
        for phase in 0..8 {
            let (s, _) = one(rate, phase, 0.0, 0.0, None, 0x100 + phase as u64);
            row.push_str(&format!("  {}", cell(s)));
            if s.parts < PARTS {
                failed.push((name(rate), phase, s.parts));
            }
        }
        println!("{row}");
    }
    assert!(failed.is_empty(), "lost: {failed:?}");
}

/// Clause 4's seven hertz either way with the far clock 200 ppm out either
/// way, clean and at 30 dB.
#[test]
fn seven_hertz_and_two_hundred_ppm() {
    let mut failed = Vec::new();
    for rate in RATES {
        for snr in [None, Some(30.0)] {
            let mut row = format!("{:>5} {:>6}:", name(rate), snr.map_or("clean".to_owned(), |s| format!("{s} dB")));
            for (k, (hz, ppm)) in [(7.0, 200.0), (-7.0, -200.0), (7.0, -200.0), (-7.0, 200.0)].into_iter().enumerate() {
                for phase in [0, 3] {
                    let (s, _) = one(rate, phase, hz, ppm, snr, 0x200 + 8 * k as u64 + phase as u64);
                    row.push_str(&format!("  {hz:+}/{ppm:+} {}", cell(s)));
                    if s.parts < PARTS {
                        failed.push((name(rate), snr, hz, ppm, phase, s.parts));
                    }
                }
            }
            println!("{row}");
        }
    }
    assert!(failed.is_empty(), "lost: {failed:?}");
}

/// The signal to noise each rate works at, and the question fax-qam 6.3
/// left open: whether 7200 is worse than 9600 once noise is what limits it.
///
/// Noise from the burst's first sample, so that segment 1's twenty
/// milliseconds are the only line the receiver hears before the signal.
/// Four bursts at each ratio. A rate works down to the lowest ratio at
/// which, and at every ratio above which, every eighth of all four came
/// back: a bit error rate of about 1e-5 or better. 7200's points are 3.9 dB
/// further apart than 9600's for the same power, so at the same ratio it
/// should make many times fewer errors, not more.
#[test]
fn noise_at_each_rate() {
    const SEEDS: u64 = 4;
    let sweeps: [(Rate, &[f64]); 3] = [
        (Rate::R9600, &[14.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 24.0, 30.0]),
        (Rate::R7200, &[10.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 20.0, 26.0]),
        (Rate::R4800, &[6.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 16.0, 20.0]),
    ];
    let mut works = Vec::new();
    let mut rates = Vec::new();
    for (rate, snrs) in sweeps {
        let mut row = format!("{:>5}:", name(rate));
        let mut lowest = None;
        let mut broken = false;
        for &snr in snrs.iter().rev() {
            let (mut parts, mut ber) = (0, 0.0);
            for seed in 0..SEEDS {
                let (s, _) = one(rate, 0, 0.0, 0.0, Some(snr), 0x300 + seed + (snr as u64) * 16);
                parts += s.parts;
                ber += s.ber / SEEDS as f64;
            }
            row.push_str(&format!("  {snr} dB {parts}/{} {ber:.0e}", PARTS as u64 * SEEDS));
            broken |= parts < PARTS * SEEDS as usize;
            if !broken {
                lowest = Some(snr);
            }
            rates.push((rate, snr, ber));
        }
        println!("{row}");
        println!("{:>5}: works down to {}", name(rate), lowest.map_or("nowhere".to_owned(), |l| format!("{l} dB")));
        works.push((rate, lowest));
    }
    let at = |rate| works.iter().find(|w| w.0 == rate).and_then(|w| w.1).unwrap_or(f64::INFINITY);
    let ber = |rate, snr| rates.iter().find(|r| r.0 == rate && r.1 == snr).map_or(0.5, |r| r.2);
    assert!(at(Rate::R9600) <= 20.0, "9600 needs {} dB", at(Rate::R9600));
    assert!(at(Rate::R7200) <= 18.0, "7200 needs {} dB", at(Rate::R7200));
    assert!(at(Rate::R4800) <= 14.0, "4800 needs {} dB", at(Rate::R4800));
    for snr in [16.0, 18.0] {
        assert!(
            ber(Rate::R7200, snr) * 10.0 <= ber(Rate::R9600, snr),
            "at {snr} dB 7200 makes {:.0e} of errors and 9600 {:.0e}",
            ber(Rate::R7200, snr),
            ber(Rate::R9600, snr)
        );
    }
}

/// A twenty-millisecond concealment in the middle of the data, and inside
/// segment 2: 320 samples of silence put in, the 320 before repeated, or 320
/// taken out, as a VoIP jitter buffer does.
#[test]
fn a_twenty_millisecond_slip() {
    const SLIP: usize = 320;
    let mut failed = Vec::new();
    for rate in RATES {
        let data = payload(0x400);
        let sent = burst(rate, &data);
        let power = burst_power(&sent);
        let seg2 = to_segment_2();
        let data_at = seg2 + ((128.0 + 384.0 + 48.0) * FS / BAUD) as usize;
        // Where: a third of the way into the data, and 20, 50 and 80 symbols
        // into segment 2.
        let places = [
            ("data", data_at + (sent.len() - data_at) / 3),
            ("seg2+20", seg2 + (20.0 * FS / BAUD) as usize),
            ("seg2+50", seg2 + (50.0 * FS / BAUD) as usize),
            ("seg2+80", seg2 + (80.0 * FS / BAUD) as usize),
        ];
        for snr in [None, Some(30.0)] {
            let mut row = format!("{:>5} {:>6}:", name(rate), snr.map_or("clean".to_owned(), |s| format!("{s} dB")));
            for (place, at) in places {
                for kind in ["silence", "repeat", "drop"] {
                    let mut line: Vec<f64> = sent[..at].to_vec();
                    match kind {
                        "silence" => {
                            line.extend(std::iter::repeat_n(0.0, SLIP));
                            line.extend_from_slice(&sent[at..]);
                        }
                        "repeat" => {
                            line.extend_from_slice(&sent[at - SLIP..at]);
                            line.extend_from_slice(&sent[at..]);
                        }
                        _ => line.extend_from_slice(&sent[at + SLIP..]),
                    }
                    line.extend(std::iter::repeat_n(0.0, TAIL));
                    if let Some(snr) = snr {
                        add_noise(&mut line, sigma(power, snr), 0x401 + at as u64);
                    }
                    let heard = listen(rate, &line, &[], 0..0);
                    let s = score(&data, &heard.bits);
                    row.push_str(&format!("  {place} {kind} {}", s.parts));
                    // In the data it costs the eighths it lands in, at most
                    // two; in segment 2 it must cost nothing.
                    let allowed = if place == "data" { PARTS - 2 } else { PARTS };
                    if s.parts < allowed {
                        failed.push((name(rate), snr, place, kind, s.parts));
                    }
                }
            }
            println!("{row}");
        }
    }
    assert!(failed.is_empty(), "lost: {failed:?}");
}

/// Two seconds of line noise, and then the burst (fax-qam 3.1): the
/// carrier must not be heard in the noise, and the burst must be read.
#[test]
fn a_burst_after_a_long_quiet_stretch_of_line_noise() {
    const QUIET: usize = 32_000;
    let mut failed = Vec::new();
    for (rate, snrs) in [(Rate::R9600, [30.0, 22.0]), (Rate::R7200, [30.0, 18.0]), (Rate::R4800, [30.0, 14.0])] {
        let mut row = format!("{:>5}:", name(rate));
        for snr in snrs {
            for seed in 0..2 {
                let (s, heard) = one(rate, QUIET, 0.0, 0.0, Some(snr), 0x500 + seed + snr as u64);
                let noise_ms = heard.on_in as f64 / FS * 1000.0;
                row.push_str(&format!("  {snr} dB {} carrier in the noise {noise_ms:.0} ms", cell(s)));
                if s.parts < PARTS || heard.on_in > 0 {
                    failed.push((name(rate), snr, seed, s.parts, heard.on_in));
                }
            }
        }
        println!("{row}");
    }
    assert!(failed.is_empty(), "lost: {failed:?}");
}

/// Protection against talker echo in front of the burst, as V.27 ter 5.2.1
/// has it and a real fax service sends it (fax-qam 3.4): 200 ms of
/// unmodulated carrier at the burst's own power, 20 ms of nothing, then the
/// burst. One carrier, not two, and the burst read.
#[test]
fn talker_echo_protection_in_front_of_the_burst() {
    let mut failed = Vec::new();
    for rate in RATES {
        let mut row = format!("{:>5}:", name(rate));
        for (tone, snr) in [(CARRIER, None), (CARRIER, Some(30.0)), (1800.0, None)] {
            let data = payload(0x600);
            let sent = burst(rate, &data);
            let power = burst_power(&sent);
            let amplitude = (2.0 * power).sqrt();
            let mut line: Vec<f64> = (0..3200).map(|n| amplitude * (TAU * tone * n as f64 / FS).cos()).collect();
            line.extend(std::iter::repeat_n(0.0, 320));
            let burst_at = line.len();
            line.extend_from_slice(&sent);
            line.extend(std::iter::repeat_n(0.0, TAIL));
            if let Some(snr) = snr {
                add_noise(&mut line, sigma(power, snr), 0x601);
            }
            let heard = listen(rate, &line, &[], 0..burst_at);
            let s = score(&data, &heard.bits);
            row.push_str(&format!(
                "  {tone} Hz {}: {} carriers, {:.0} ms in front, {}",
                snr.map_or("clean".to_owned(), |s| format!("{s} dB")),
                heard.rises.len(),
                heard.on_in as f64 / FS * 1000.0,
                cell(s)
            ));
            if heard.rises.len() != 1 || heard.on_in > 0 || s.parts < PARTS {
                failed.push((name(rate), tone, snr, heard.rises.len(), heard.on_in, s.parts));
            }
        }
        println!("{row}");
    }
    assert!(failed.is_empty(), "lost: {failed:?}");
}

/// Bursts one after another as a fax call hears them: a training check,
/// then a page, then another, with the line turned round between them. With
/// the receiver restarted for each, as `modem::faxcall` does, and without,
/// as a bare receiver left listening hears them; on a clean line and with
/// noise on it throughout.
#[test]
fn several_bursts_in_a_row() {
    let mut failed = Vec::new();
    for rate in RATES {
        for snr in [None, Some(30.0)] {
            for restarting in [true, false] {
                let payloads = [payload(0x700), payload(0x701), payload(0x702)];
                let mut line = vec![0.0; 1600];
                let mut restarts = Vec::new();
                let mut power = 0.0;
                for data in &payloads {
                    restarts.push(line.len() - 800);
                    let sent = burst(rate, data);
                    power = burst_power(&sent);
                    line.extend_from_slice(&sent);
                    // The turn-round: the far end's silence while this end
                    // answers on V.21, which this receiver is not fed.
                    line.extend(std::iter::repeat_n(0.0, 1200));
                }
                line.extend(std::iter::repeat_n(0.0, TAIL));
                if let Some(snr) = snr {
                    add_noise(&mut line, sigma(power, snr), 0x703);
                }
                let heard = listen(rate, &line, if restarting { &restarts } else { &[] }, 0..0);
                let scores: Vec<Score> = payloads.iter().map(|d| score(d, &heard.bits)).collect();
                println!(
                    "{:>5} {:>6} {}: {} carriers, {}",
                    name(rate),
                    snr.map_or("clean".to_owned(), |s| format!("{s} dB")),
                    if restarting { "restarted" } else { "left on " },
                    heard.rises.len(),
                    scores.iter().map(|s| cell(*s)).collect::<Vec<_>>().join("  ")
                );
                if heard.rises.len() != payloads.len() || scores.iter().any(|s| s.parts < PARTS) {
                    failed.push((name(rate), snr, restarting, heard.rises.len()));
                }
            }
        }
    }
    assert!(failed.is_empty(), "lost: {failed:?}");
}

/// The line with one echo of itself on it, as fax-qam 3.5 measured the
/// receiver before this one against: a fifth, a third and a half as loud,
/// 0.44, 0.88 and 1.3 ms late, at 30 dB. A half at 0.88 ms puts a 6 dB notch
/// at 1705 Hz, on the carrier. The equaliser is solved for on segment 3, so
/// nothing has to open the eye blind.
#[test]
fn a_line_with_an_echo_on_it() {
    let mut failed = Vec::new();
    for rate in RATES {
        let mut row = format!("{:>5}:", name(rate));
        for (gain, delay) in [(0.2, 7), (0.35, 7), (0.5, 7), (0.2, 14), (0.35, 14), (0.5, 14), (0.2, 21)] {
            let data = payload(0x800 + delay as u64);
            let sent = burst(rate, &data);
            let mut line: Vec<f64> = (0..sent.len() + delay)
                .map(|n| {
                    let direct = sent.get(n).copied().unwrap_or(0.0);
                    let echo = n.checked_sub(delay).and_then(|m| sent.get(m)).copied().unwrap_or(0.0);
                    direct + gain * echo
                })
                .collect();
            let power = burst_power(&line);
            line.extend(std::iter::repeat_n(0.0, TAIL));
            add_noise(&mut line, sigma(power, 30.0), 0x801 + delay as u64);
            let heard = listen(rate, &line, &[], 0..0);
            let s = score(&data, &heard.bits);
            row.push_str(&format!("  {gain} at {:.2} ms {}", delay as f64 / FS * 1000.0, cell(s)));
            if s.parts < PARTS {
                failed.push((name(rate), gain, delay, s.parts));
            }
        }
        println!("{row}");
    }
    assert!(failed.is_empty(), "lost: {failed:?}");
}

