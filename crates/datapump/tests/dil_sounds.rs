//! Render a V.90 DIL descriptor as it sounds on the line, and check it still
//! does a DIL's job.
//!
//! Ignored: it is a design tool. Give it a spec and somewhere to put the
//! sound:
//!
//! ```text
//! DIL_SPEC=spec.txt DIL_WAV=out.wav cargo test -p datapump --release \
//!     --test dil_sounds -- --ignored --nocapture
//! ```
//!
//! A spec is lines of `key: value`, or the single word `baseline` for the DIL
//! this modem asks for today:
//!
//! ```text
//! signs: ++--+-...     SP, 1 to 128 of + and -
//! training: 000000111  TP, 1 to 128 of 0 (reference) and 1 (training)
//! h: 5 5 5 5 5 5 5 5   H1 to H8, 0 to 15: a segment in Uchord c is (Hc+1)*6
//! order: 0 3 6 ...     the training Ucodes, in the order they are sent
//! passes: 2            how many passes to render (default 2)
//! ```
//!
//! `order` must hold every Ucode up to a third of full scale but UINFO (79)
//! exactly once. The report says what the DIL measures, next to today's.

use datapump::v90::dil::{self, Analysis, Route, LOUDEST};
use datapump::v90::encoder::Mapping;
use datapump::v90::sequences::Descriptor;
use datapump::v90::ucode::{self, Law};

const UINFO: u8 = 79;
const INTERVALS: usize = 6;

fn parse(text: &str) -> Result<(Descriptor, usize), String> {
    if text.trim() == "baseline" {
        return Ok((dil::design(Law::Mu, UINFO), 2));
    }
    let mut signs = None;
    let mut training = None;
    let mut h = None;
    let mut order = None;
    let mut passes = 2;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (key, value) = line.split_once(':').ok_or_else(|| format!("not key: value: {line}"))?;
        let value = value.trim();
        match key.trim() {
            "signs" => {
                signs = Some(
                    value
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .map(|c| match c {
                            '+' => Ok(true),
                            '-' => Ok(false),
                            other => Err(format!("sign {other:?}")),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            "training" => {
                training = Some(
                    value
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .map(|c| match c {
                            '1' => Ok(true),
                            '0' => Ok(false),
                            other => Err(format!("training {other:?}")),
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            "h" => {
                let v: Vec<u8> = value.split_whitespace().map(|n| n.parse().map_err(|e| format!("h: {e}"))).collect::<Result<_, _>>()?;
                let v: [u8; 8] = v.try_into().map_err(|_| "h wants eight numbers".to_owned())?;
                h = Some(v);
            }
            "order" => {
                order = Some(
                    value
                        .split(|c: char| c.is_whitespace() || c == ',')
                        .filter(|s| !s.is_empty())
                        .map(|n| n.parse::<u8>().map_err(|e| format!("order: {e}")))
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
            "passes" => passes = value.parse().map_err(|e| format!("passes: {e}"))?,
            other => return Err(format!("unknown key {other}")),
        }
    }
    let base = dil::design(Law::Mu, UINFO);
    let d = Descriptor {
        signs: signs.unwrap_or(base.signs),
        training: training.unwrap_or(base.training),
        h: h.unwrap_or(base.h),
        refs: [UINFO; 8],
        ucodes: order.unwrap_or(base.ucodes),
    };
    Ok((d, passes))
}

/// What is wrong with a descriptor as a DIL this modem could ask for.
fn problems(d: &Descriptor) -> Vec<String> {
    let mut out = Vec::new();
    if !(1..=128).contains(&d.signs.len()) {
        out.push(format!("SP is {} long", d.signs.len()));
    }
    if !(1..=128).contains(&d.training.len()) {
        out.push(format!("TP is {} long", d.training.len()));
    }
    if d.h.iter().any(|&h| h > 15) {
        out.push("an H above 15".to_owned());
    }
    let mut wanted: Vec<u8> = (0..128u8).filter(|&u| u != UINFO && ucode::level(Law::Mu, u) <= LOUDEST).collect();
    let mut got = d.ucodes.clone();
    got.sort_unstable();
    wanted.sort_unstable();
    if got != wanted {
        out.push(format!("order is not every Ucode 0..=98 but {UINFO} once: {} given", d.ucodes.len()));
    }
    if let Some(w) = d.ucodes.windows(2).find(|w| w[0].abs_diff(w[1]) < 3) {
        out.push(format!("neighbours {} and {} are closer than 3 apart", w[0], w[1]));
    }
    if Descriptor::from_bits(&d.to_bits()).as_ref() != Some(d) {
        out.push("does not survive being sent".to_owned());
    }
    for &u in &d.ucodes {
        let len = d.segment_length(u);
        let signs = (0..len).map(|n| d.signs[n % d.signs.len()]).filter(|b| *b).count();
        if signs * 3 < len || signs * 3 > 2 * len {
            out.push(format!("a Ucode {u} segment ({len} symbols) has {signs} positive"));
            break;
        }
        // Every interval read at least once in every segment.
        let trained: Vec<bool> = (0..INTERVALS)
            .map(|i| (0..len).filter(|n| n % INTERVALS == i).any(|n| d.training[n % d.training.len()]))
            .collect();
        if trained.iter().any(|t| !t) {
            out.push(format!("a Ucode {u} segment ({len} symbols) never trains some interval"));
            break;
        }
        if !(0..len).any(|n| !d.training[n % d.training.len()]) {
            out.push(format!("a Ucode {u} segment has no reference"));
            break;
        }
    }
    out
}

/// The DIL through a route: noise, and optionally a robbed bit in interval
/// 3, read as the analogue modem reads it.
fn analyse(d: &Descriptor, noise: f64, rob: bool) -> Route {
    let mut analysis = Analysis::new();
    let mut x = 0x1234_5678_9abc_def0u64;
    for (n, (u, positive)) in d.symbols().enumerate() {
        let interval = n % INTERVALS;
        let arrived = if rob && interval == 3 {
            let octet = ucode::octet(Law::Mu, u, false) | 1;
            ucode::from_octet(Law::Mu, octet).0
        } else {
            u
        };
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let gaussian = ((x >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 12f64.sqrt() * noise;
        let level = ucode::level(Law::Mu, arrived) * if positive { 1.0 } else { -1.0 };
        analysis.feed(u, positive, interval, level + gaussian);
    }
    analysis.route()
}

fn rate(route: &Route) -> u32 {
    dil::choose(route, Law::Mu, 15124, |_| true)
        .and_then(|c| Mapping::from_cp(&c.data).map(|m| m.rate()))
        .unwrap_or(0)
}

/// Share of the power within 40 Hz of `f`.
fn share_near(samples: &[f64], f: f64) -> f64 {
    let n = samples.len();
    let total: f64 = samples.iter().map(|s| s * s).sum::<f64>() * n as f64 / 2.0;
    let mut near = 0.0;
    let bins = (n as f64 * 40.0 / 8000.0).ceil() as i64;
    let centre = (f * n as f64 / 8000.0).round() as i64;
    for k in centre - bins..=centre + bins {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, s) in samples.iter().enumerate() {
            let a = std::f64::consts::TAU * k as f64 * i as f64 / n as f64;
            re += s * a.cos();
            im += s * a.sin();
        }
        near += re * re + im * im;
    }
    near / total.max(1e-12)
}

#[test]
#[ignore = "a design tool; see the module comment"]
fn render_a_dil() {
    let (Ok(spec), Ok(out)) = (std::env::var("DIL_SPEC"), std::env::var("DIL_WAV")) else {
        println!("set DIL_SPEC and DIL_WAV");
        return;
    };
    let text = std::fs::read_to_string(&spec).expect("could not read the spec");
    let (d, passes) = parse(&text).unwrap_or_else(|e| panic!("spec: {e}"));
    let issues = problems(&d);
    let pass: Vec<f64> = d
        .symbols()
        .map(|(u, positive)| ucode::level(Law::Mu, u) * if positive { 1.0 } else { -1.0 })
        .collect();
    let seconds = pass.len() as f64 / 8000.0;
    let base = dil::design(Law::Mu, UINFO);
    let mut report = Vec::new();
    for (noise, rob) in [(0.0005, false), (0.002, false), (0.002, true), (0.004, true)] {
        let ours = analyse(&d, noise, rob);
        let today = analyse(&base, noise, rob);
        report.push(format!(
            "noise {noise} robbed {rob}: {} bit/s (today {}), noise read {:.5} (today {:.5})",
            rate(&ours),
            rate(&today),
            ours.noise(),
            today.noise()
        ));
    }
    let mut samples: Vec<f32> = Vec::new();
    for _ in 0..passes.max(1) {
        samples.extend(pass.iter().map(|&s| s as f32));
    }
    datapump_line_write(&out, &samples);
    println!("RESULT segments {} symbols {} seconds {seconds:.3} problems {:?}", d.ucodes.len(), pass.len(), issues);
    println!("RESULT near 2100 Hz {:.4} near 2600 Hz {:.4}", share_near(&pass, 2100.0), share_near(&pass, 2600.0));
    for r in report {
        println!("RESULT {r}");
    }
}

fn datapump_line_write(path: &str, samples: &[f32]) {
    line::wav::write(path, samples, 8000).expect("could not write the sound");
}
