//! Replay a recorded V.32 call through this modem's own start-up.
//!
//! Ignored, because it needs a capture.
//!
//! ```text
//! V32_CAPTURE=F:/dialupmodem2/dist/captures/live-1788836496.wav \
//!     cargo test -p datapump --test v32_replay -- --ignored --nocapture
//! ```
//!
//! Channel 0 of a live recording is what came back off the line: the far
//! modem, plus whatever the network returned of our own signal. Feeding it to
//! a `Modem` is the call happening again, with every decision this end made
//! visible and repeatable -- which a live call is not.
//!
//! What the transmitter produces is thrown away. It cannot be put back on the
//! line, so the far end in the recording is answering the call that was made
//! rather than this one; the replay is faithful up to the first point where
//! the two would differ, and that point is what is being looked for.

use datapump::v32::startup::{Rates, Modem, Role, Status, rate_signal};
use datapump::v32::Coding;
use datapump::v32::trellis::{AT_7200, AT_9600, AT_12000, AT_14400};

const FS: f64 = 16_000.0;

/// A B C D of Figure 1/V.32, which 4800 and every training segment use.
const FOUR: [(f64, f64); 4] = [(-3.0, -1.0), (1.0, -3.0), (3.0, 1.0), (-1.0, 3.0)];
/// Figure 2/V.32: 9600's non-redundant alternative (2.4.1.1).
const SIXTEEN: [(f64, f64); 16] = [
    (-3.0, -3.0), (-3.0, -1.0), (-3.0, 1.0), (-3.0, 3.0),
    (-1.0, -3.0), (-1.0, -1.0), (-1.0, 1.0), (-1.0, 3.0),
    (1.0, -3.0), (1.0, -1.0), (1.0, 1.0), (1.0, 3.0),
    (3.0, -3.0), (3.0, -1.0), (3.0, 1.0), (3.0, 3.0),
];

fn nearest_of(points: &[(f64, f64)], (i, q): (f64, f64)) -> (f64, f64) {
    *points
        .iter()
        .min_by(|a, b| {
            let d = |p: &(f64, f64)| (i - p.0).powi(2) + (q - p.1).powi(2);
            d(a).total_cmp(&d(b))
        })
        .expect("no points")
}

fn rate_of(status: Status) -> Option<u32> {
    match status {
        Status::Connected(rate) => Some(rate),
        _ => None,
    }
}

/// The closest two points of whatever is in use, for scale.
fn spacing(status: Status, coding: Coding) -> f64 {
    match (rate_of(status), coding) {
        (Some(7200), _) => AT_7200.closest(),
        (Some(9600), Coding::Trellis) => AT_9600.closest(),
        (Some(12_000), _) => AT_12000.closest(),
        (Some(14_400), _) => AT_14400.closest(),
        // Figure 2/V.32's sixteen sit on a grid of two.
        (Some(9600), _) => 2.0,
        // A B C D are a knight's move apart on the same grid.
        _ => f64::sqrt(20.0),
    }
}

#[test]
#[ignore = "needs a capture"]
fn what_this_end_made_of_it() {
    let path = std::env::var("V32_CAPTURE").expect("set V32_CAPTURE");
    let offer = std::env::var("V32_OFFER").ok();
    let wav = line::wav::read(&path).expect("could not read the capture");
    assert_eq!(wav.sample_rate as f64, FS, "built for 16 kHz");

    // Channel 0 is what came off the line. Channel 1 is what this modem put
    // on it, which is worth replaying too: it is clean, it has no echo in it,
    // and reading it back says what this end actually asked for rather than
    // what it meant to. It has to be read as the far end would, which means
    // the other role -- 4.1 gives each direction its own scrambler.
    let channel: usize = std::env::var("V32_CHANNEL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let role = if channel == 0 { Role::Calling } else { Role::Answering };
    let arrived = wav.channel(channel);

    // Where in the recording to start the modem, which is not the beginning.
    // A live call builds this pump only once V.8 has agreed on it, so before
    // that moment nothing here has heard anything -- while a replay from the
    // top hands it the dial tone, the ringing and whatever else the network
    // played over a call nobody had picked up yet, and an adaptive receiver
    // let loose on all that keeps what it learns. Measured on one capture:
    // eight seconds of call progress before the far end answered pinned the
    // residual error at 0.39 for the remaining forty, through a conditioning
    // signal and a training segment that should have fixed anything, and the
    // far end's rate signal arrived 356 times and was read none of them. The
    // same modem started a few seconds later read all 356. That is a fault in
    // the replay and not in the modem, and a replay that invents faults is
    // worse than no replay.
    let from: f64 = std::env::var("V32_FROM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let arrived = &arrived[((from * FS) as usize).min(arrived.len())..];
    let offer = match offer.as_deref() {
        Some("4800") => rate_signal(Rates { at_4800: true, ..Rates::default() }),
        Some("9600") => rate_signal(Rates { at_9600: true, ..Rates::default() }),
        Some("all") => rate_signal(Rates::between(4800, 14_400)),
        _ => rate_signal(Rates { at_4800: true, at_9600: true, ..Rates::default() }),
    };
    println!(
        "\n{path}: {:.1} s, channel {channel} as the {} end, offering {offer:016b}\n",
        wav.duration_secs(),
        if channel == 0 { "calling" } else { "answering" }
    );
    println!("started at {from:.2} s into it\n");

    let mut modem = Modem::new(role, offer, FS);
    let mut phase = "";
    let mut status = Status::Negotiating;
    let mut carrier = false;
    let mut bytes = Vec::new();
    let mut points: Vec<f64> = Vec::new();
    // Time, where the symbol landed, and what the equaliser was left with
    // at that moment -- which is the number 7's retrain decides on, so it has
    // to be recorded as the call goes and not read off the wreckage
    // afterwards.
    let mut lock: Vec<(f64, (f64, f64), f64)> = Vec::new();

    for (i, &x) in arrived.iter().enumerate() {
        let _ = modem.step(f64::from(x));
        let at = from + i as f64 / FS;
        if modem.phase() != phase {
            phase = modem.phase();
            println!("{at:8.3}s  phase {phase}");
        }
        if modem.status() != status {
            status = modem.status();
            println!("{at:8.3}s  status {status:?}");
        }
        if modem.carrier() != carrier {
            carrier = modem.carrier();
            println!("{at:8.3}s  carrier {carrier}");
        }
        for seq in modem.take_sequences() {
            println!(
                "{at:8.3}s  heard {}",
                datapump::v32::startup::describe_sequence(seq)
            );
        }
        if matches!(status, Status::Connected(_)) {
            bytes.extend(modem.take_bytes());
            // Normalised to unit mean power by the accessor; the tables
            // are in the Recommendation's units, so put it back.
            let (i, q) = modem.constellation_point();
            let (i, q) = (
                i * datapump::v32::CONSTELLATION_RMS,
                q * datapump::v32::CONSTELLATION_RMS,
            );
            if i != 0.0 || q != 0.0 {
                points.push((i * i + q * q).sqrt());
                lock.push((at, (i, q), modem.residual_error()));
            }
        }
    }

    println!(
        "\nround trip {} symbols, echo return loss {:.1} dB",
        modem.round_trip(),
        modem.echo_return_loss()
    );
    let printable = bytes
        .iter()
        .filter(|c| (32..127).contains(*c) || **c == 10 || **c == 13)
        .count();
    println!(
        "{} octets after connecting, {printable} of them printable",
        bytes.len()
    );
    let text: String = bytes
        .iter()
        .take(400)
        .map(|&c| if (32..127).contains(&c) || c == 10 || c == 13 { c as char } else { '.' })
        .collect();
    println!("{text}");

    // How far each symbol lands from the nearest point it could have been, a
    // second at a time. A receiver that has locked sits close to one; one
    // whose carrier is turning wanders the whole constellation and averages
    // out around the spacing itself.
    if !lock.is_empty() {
        println!("
  second  symbols  mean miss   error along vs across the radius");
        let mut second = (lock[0].0 * 10.0).floor();
        let (mut n, mut sum) = (0usize, 0.0);
        let (mut radial, mut tangential) = (0.0f64, 0.0f64);
        let (mut dot, mut cross) = (0.0f64, 0.0f64);
        let (mut residual, mut worst) = (0.0f64, 0.0f64);
        for &(at, (i, q), left) in &lock {
            if (at * 10.0).floor() != second {
                if n > 0 {
                    let miss = sum / n as f64;
                    let across = (tangential / n as f64).sqrt();
                    let along = (radial / n as f64).sqrt();
                    let snr = 10.0
                        * (10.0 / ((radial + tangential) / n as f64).max(1e-12))
                            .log10();
                    // The miss on its own means nothing without the
                    // constellation it was measured in: 0.35 is a working
                    // receiver at 4800 and a dead one at 14 400, where the
                    // closest two points are a fifth as far apart. As a
                    // fraction of that distance it means the same thing at
                    // every rate, and half of it is where a decision is as
                    // likely to be wrong as right.
                    let gap = spacing(status, modem.coding());
                    let residual = residual / n as f64;
                    println!(
                        "  {:7.1}s  miss {miss:6.3} = {:5.2} of the gap  SNR {snr:5.1} dB  \
                         turn {:+7.2} deg  residual {residual:5.3} worst {worst:5.3}",
                        second / 10.0,
                        miss / gap,
                        cross.atan2(dot).to_degrees(),
                    );
                    let _ = (along, across);
                }
                second = (at * 10.0).floor();
                n = 0;
                sum = 0.0;
                radial = 0.0;
                tangential = 0.0;
                dot = 0.0;
                cross = 0.0;
                residual = 0.0;
                worst = 0.0;
            }
            residual += left;
            worst = worst.max(left);
            // Whichever constellation the call settled on. Reading a
            // hundred and twenty-eight points against the four of 4800 puts
            // every symbol most of a quadrant from its answer and reports a
            // working receiver as noise, which is what this did until a call
            // came up at 14 400 and the table said 5 dB while the modem was
            // decoding it.
            let (x, y) = match (rate_of(status), modem.coding()) {
                (Some(7200), _) => AT_7200.point(AT_7200.nearest((i, q))),
                (Some(9600), Coding::Trellis) => {
                    AT_9600.point(AT_9600.nearest((i, q)))
                }
                (Some(12_000), _) => AT_12000.point(AT_12000.nearest((i, q))),
                (Some(14_400), _) => AT_14400.point(AT_14400.nearest((i, q))),
                // 4800, and 9600's uncoded modulation, which 2.4.1.1 puts on
                // the sixteen points of Figure 2/V.32 rather than a coded set.
                (Some(9600), _) => nearest_of(&SIXTEEN, (i, q)),
                _ => nearest_of(&FOUR, (i, q)),
            };
            sum += ((i - x).powi(2) + (q - y).powi(2)).sqrt();
            // Split the error into the part along the radius and the part
            // across it. Additive noise is the same in both; a constellation
            // being turned is all across.
            let r = (x * x + y * y).sqrt().max(1e-9);
            let (ex, ey) = (i - x, q - y);
            radial += ((ex * x + ey * y) / r).powi(2);
            tangential += ((ey * x - ex * y) / r).powi(2);
            dot += i * x + q * y;
            cross += q * x - i * y;
            n += 1;
        }
    }

    // One number to compare settings by: how much of the connected time the
    // constellation was clean enough for thirty-two points to be readable.
    if !lock.is_empty() {
        let (mut good, mut total) = (0usize, 0usize);
        let (mut n, mut sq) = (0usize, 0.0f64);
        let mut window = (lock[0].0 * 10.0).floor();
        for &(at, p, _) in &lock {
            if (at * 10.0).floor() != window {
                if n > 0 {
                    let snr = 10.0 * (10.0 / (sq / n as f64).max(1e-12)).log10();
                    total += 1;
                    if snr > 24.0 {
                        good += 1;
                    }
                }
                window = (at * 10.0).floor();
                n = 0;
                sq = 0.0;
            }
            let (x, y) = AT_9600.point(AT_9600.nearest(p));
            sq += (p.0 - x).powi(2) + (p.1 - y).powi(2);
            n += 1;
        }
        println!(
            "SUMMARY {:.1}s of {:.1}s above 24 dB ({:.0}%)",
            good as f64 / 10.0,
            total as f64 / 10.0,
            100.0 * good as f64 / total.max(1) as f64
        );
    }

    // Is what is left of each symbol noise, or the symbols either side of it
    // leaking in? Additive noise is uncorrelated with any of them; what a
    // short equaliser leaves behind is the neighbours, and shows up here.
    if lock.len() > 1000 {
        let decided: Vec<(f64, f64)> = lock
            .iter()
            .map(|&(_, p, _)| AT_9600.point(AT_9600.nearest(p)))
            .collect();
        println!("
  error against the symbol at each lag (0 is itself):");
        for lag in -3i32..=3 {
            let (mut num, mut ee, mut dd) = (0.0f64, 0.0f64, 0.0f64);
            for n in 4..lock.len() - 4 {
                let (i, q) = lock[n].1;
                let (x, y) = decided[n];
                let (ex, ey) = (i - x, q - y);
                let (dx, dy) = decided[(n as i32 + lag) as usize];
                num += ex * dx + ey * dy;
                ee += ex * ex + ey * ey;
                dd += dx * dx + dy * dy;
            }
            let r = num / (ee * dd).sqrt().max(1e-12);
            let bar = "#".repeat((r.abs() * 200.0).min(40.0) as usize);
            println!("  {lag:+3}   {r:+8.4}  {bar}");
        }
    }

    // The scale error, ring by ring. A gain that is simply wrong shrinks every
    // ring by the same fraction. Something in the path compressing the loud
    // symbols -- a codec, a softphone's own gain control, a limiter -- pulls
    // the outer rings in and leaves the inner ones alone, and that is fatal at
    // thirty-two points and invisible at four, where every symbol has the same
    // amplitude.
    if lock.len() > 1000 {
        println!("
  ideal radius   symbols   measured   ratio");
        let mut rings: std::collections::BTreeMap<i64, (usize, f64)> =
            std::collections::BTreeMap::new();
        for &(_, p, _) in &lock {
            let (x, y) = AT_9600.point(AT_9600.nearest(p));
            let ideal = (x * x + y * y).sqrt();
            let got = (p.0 * p.0 + p.1 * p.1).sqrt();
            let e = rings.entry((ideal * 1000.0).round() as i64).or_insert((0, 0.0));
            e.0 += 1;
            e.1 += got;
        }
        for (k, (n, sum)) in rings {
            let ideal = k as f64 / 1000.0;
            let got = sum / n as f64;
            println!(
                "  {ideal:12.3}   {n:7}   {got:8.3}   {:5.3}",
                got / ideal
            );
        }
    }

    // How many amplitude rings the far end's constellation has. The 16-point
    // non-redundant alternative at 9600 puts its points at radii root 2, root
    // 10 and root 18 -- three rings. The 32-point trellis cross has five.
    if !points.is_empty() {
        let mean = points.iter().sum::<f64>() / points.len() as f64;
        let mut hist = [0usize; 40];
        for r in &points {
            let bin = ((r / mean) * 10.0) as usize;
            if bin < hist.len() {
                hist[bin] += 1;
            }
        }
        println!("
radius, against the mean ({} symbols):", points.len());
        let peak = *hist.iter().max().unwrap_or(&1) as f64;
        for (bin, &n) in hist.iter().enumerate() {
            if n * 200 > points.len() {
                println!(
                    "  {:.1}  {:6}  {}",
                    bin as f64 / 10.0,
                    n,
                    "#".repeat((n as f64 / peak * 50.0) as usize)
                );
            }
        }
    }
}
