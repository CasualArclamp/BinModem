//! The received constellation of every mode, over a line with noise on it.
//!
//! Two modems call each other through a simulated line, and the receiving
//! end's symbol-scope points are written out, one folder for each mode:
//!
//! ```text
//! cargo run --release -p modem --example constellations -- docs/img/constellations
//! python tools/constellations.py docs/img/constellations
//! ```
//!
//! The second line draws the PNGs. A name after the folder runs only the
//! modes whose folder name contains it. The noise is seeded, so a second run
//! gives the same points.

use datapump::v90::network::Network;
use datapump::v90::ucode::Law;
use fax::call::Phase;
use fax::page::{Page, Resolution};
use fax::t30::Modulation;
use modem::{Modem, State};

const FS: f64 = 16_000.0;
const NETWORK_FS: f64 = 8000.0;

/// Xorshift64* with Box-Muller: seeded, so each run is the same.
struct Noise(u64);

impl Noise {
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((x >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

/// Noise that follows the line's own level, so `snr_db` holds whatever the
/// modulation's power is.
struct Hiss {
    noise: Noise,
    power: f64,
    snr_db: f64,
}

impl Hiss {
    fn new(snr_db: f64, seed: u64) -> Self {
        Self { noise: Noise(seed), power: 1e-6, snr_db }
    }

    /// `x` with noise added, and the level measured on `x`.
    fn add(&mut self, x: f64) -> f64 {
        self.power += 0.0002 * (x * x - self.power);
        x + (self.power / 10f64.powf(self.snr_db / 10.0)).sqrt() * self.noise.gauss()
    }
}

fn type_at(modem: &mut Modem, line: &str) {
    for b in line.bytes() {
        modem.feed_dte(b);
    }
    modem.feed_dte(b'\r');
}

/// Which end's scope is drawn.
#[derive(Clone, Copy)]
enum End {
    Caller,
    Host,
}

/// How the call is made.
enum Call {
    /// An ordinary call: `AT+MS=` this at both ends.
    Line(&'static str),
    /// A fax at one modulation, drawn at the end that receives the page.
    Fax(Modulation),
    /// V.90 through a G.711 network, drawn at the analogue end.
    Pcm,
    /// V.ARC on a stereo lead.
    Varc(&'static str),
}

struct Mode {
    /// The folder the points go in, and the picture's file name.
    slug: &'static str,
    call: Call,
    end: End,
    /// Signal to noise on the line, in dB.
    snr_db: f64,
}

const MODES: &[Mode] = &[
    Mode { slug: "v34-33600-clean", call: Call::Line("V34"), end: End::Caller, snr_db: 36.0 },
    Mode { slug: "v34-33600-noisy", call: Call::Line("V34"), end: End::Caller, snr_db: 30.0 },
    Mode { slug: "v32bis-14400", call: Call::Line("V32B"), end: End::Caller, snr_db: 30.0 },
    Mode { slug: "v32-9600", call: Call::Line("V32"), end: End::Caller, snr_db: 26.0 },
    Mode { slug: "v22bis-2400", call: Call::Line("V22B,0"), end: End::Caller, snr_db: 22.0 },
    Mode { slug: "bell212a-1200", call: Call::Line("B212,0"), end: End::Caller, snr_db: 18.0 },
    Mode { slug: "hst-14400-forward", call: Call::Line("HST"), end: End::Caller, snr_db: 30.0 },
    Mode { slug: "hst-450-back", call: Call::Line("HST"), end: End::Host, snr_db: 30.0 },
    Mode { slug: "hst-9600-forward", call: Call::Line("HST96"), end: End::Caller, snr_db: 26.0 },
    Mode { slug: "fax-v17-14400", call: Call::Fax(Modulation::V17), end: End::Host, snr_db: 30.0 },
    Mode { slug: "fax-v29-9600", call: Call::Fax(Modulation::V29), end: End::Host, snr_db: 26.0 },
    Mode { slug: "fax-v27ter-4800", call: Call::Fax(Modulation::V27ter), end: End::Host, snr_db: 22.0 },
    Mode { slug: "v90-pcm", call: Call::Pcm, end: End::Caller, snr_db: 0.0 },
    Mode { slug: "varc-qam", call: Call::Varc("VARCQ"), end: End::Caller, snr_db: 40.0 },
    Mode { slug: "varc-dmt", call: Call::Varc("VARC"), end: End::Caller, snr_db: 40.0 },
];

/// What a run gives: the points and the scope's own label for them.
struct Picture {
    points: Vec<(f64, f64)>,
    title: String,
    peak: f32,
}

fn scope(modem: &Modem) -> String {
    let rate = modem.rate().map_or(String::new(), |r| format!(" {r}"));
    // "HST 9600" names its rate already.
    let rate = if modem.standard().ends_with(rate.as_str()) { String::new() } else { rate };
    format!("{}{rate}: {} points, {}", modem.standard(), modem.states(), modem.shape())
}

/// Collect a point each time the scope's point changes.
fn take(points: &mut Vec<(f64, f64)>, modem: &Modem) {
    if let Some(z) = modem.constellation_point()
        && points.last() != Some(&z)
    {
        points.push(z);
    }
}

fn line_call(ms: &str, end: End, snr_db: f64) -> Option<Picture> {
    let (mut caller, mut host) = (Modem::new(FS), Modem::new(FS));
    type_at(&mut host, &format!("AT+MS={ms}"));
    type_at(&mut caller, &format!("AT+MS={ms}"));
    type_at(&mut host, "ATA");
    type_at(&mut caller, "ATD5551234");
    let (mut up, mut down) = (Hiss::new(snr_db, 1), Hiss::new(snr_db, 2));
    let (mut a, mut b) = (0.0, 0.0);
    let mut step = |caller: &mut Modem, host: &mut Modem| {
        let (to_caller, to_host) = (down.add(b), up.add(a));
        a = caller.step(to_caller);
        b = host.step(to_host);
        caller.take_dte();
        host.take_dte();
    };
    let connected = (0..(40.0 * FS) as usize).any(|_| {
        step(&mut caller, &mut host);
        caller.state() == State::Data && host.state() == State::Data
    });
    if !connected {
        eprintln!("  no connection: the caller is in {}", caller.line_phase());
        return None;
    }
    // Data from the host only: on HST that keeps the forward channel at the
    // host, so the caller reads the forward channel and the host the back.
    let text: Vec<u8> = (0..200_000u32).map(|i| (i * 7 + 3) as u8).collect();
    for &c in &text {
        host.feed_dte(c);
    }
    for _ in 0..(1.0 * FS) as usize {
        step(&mut caller, &mut host);
    }
    let mut points = Vec::new();
    for _ in 0..(4.0 * FS) as usize {
        step(&mut caller, &mut host);
        take(&mut points, match end {
            End::Caller => &caller,
            End::Host => &host,
        });
    }
    let drawn = match end {
        End::Caller => &caller,
        End::Host => &host,
    };
    Some(Picture { points, title: scope(drawn), peak: drawn.constellation_peak() })
}

fn a_page(lines: usize) -> Page {
    let width = fax::page::WIDTH;
    Page {
        lines: (0..lines)
            .map(|y| (0..width).map(|x| (x / 40 + y / 8).is_multiple_of(2) && x % 40 < 30).collect())
            .collect(),
        resolution: Resolution::Standard,
    }
}

fn fax_call(modulation: Modulation, snr_db: f64) -> Option<Picture> {
    let mut caller = Modem::new(FS);
    caller.fax_pages = vec![a_page(1600)];
    caller.fax_offer = vec![modulation];
    caller.fax_v34 = false;
    let mut host = Modem::new(FS);
    host.fax_offer = vec![modulation];
    host.fax_v34 = false;
    for (modem, dial) in [(&mut caller, "ATD1"), (&mut host, "ATA")] {
        type_at(modem, "AT+FCLASS=1");
        type_at(modem, dial);
    }
    let (mut up, mut down) = (Hiss::new(snr_db, 3), Hiss::new(snr_db, 4));
    let (mut a, mut b) = (0.0, 0.0);
    let mut points = Vec::new();
    let mut title = None;
    for _ in 0..(90.0 * FS) as usize {
        let (to_caller, to_host) = (down.add(b), up.add(a));
        a = caller.step(to_caller);
        b = host.step(to_host);
        caller.take_dte();
        host.take_dte();
        // The page's own modulation, not the V.21 frames around it.
        let receiving = host.fax_call().is_some_and(|c| c.phase() == Phase::Receiving) && host.states() > 2;
        if receiving {
            // The first half second is the equaliser still finding its feet.
            if title.is_none() {
                title = Some((scope(&host), host.constellation_peak(), points.len() + (0.5 * FS) as usize));
            }
            take(&mut points, &host);
            if points.len() > 12_000 {
                break;
            }
        } else if !points.is_empty() {
            break;
        }
    }
    let Some((title, peak, _)) = title else {
        eprintln!("  the page was never received: the host is in {}", host.line_phase());
        return None;
    };
    let skip = points.len().min(1200);
    Some(Picture { points: points.split_off(skip), title, peak })
}

fn pcm_call() -> Option<Picture> {
    let mut net = Network::new(Law::Mu, FS).with_delay(0.015, FS).with_noise(1e-5);
    let (mut caller, mut host) = (Modem::new(FS), Modem::new(FS));
    type_at(&mut host, "AT+MS=V90");
    type_at(&mut caller, "AT+MS=V90");
    type_at(&mut host, "ATA");
    type_at(&mut caller, "ATD5551234");
    // The digital end's samples go straight to the encoder, every other
    // one, which is how the codewords reach the network unchanged.
    let mut sent = Vec::new();
    let mut to_encoder = std::collections::VecDeque::new();
    let mut decoded = Vec::new();
    let mut resample = dsp::Resampler::new(NETWORK_FS, FS);
    let mut count = 0usize;
    let mut points = Vec::new();
    let mut data_for = 0usize;
    for _ in 0..(60.0 * NETWORK_FS) as usize {
        let from_caller = net.up(&sent);
        sent.clear();
        decoded.clear();
        resample.process(from_caller, &mut decoded);
        for &x in &decoded {
            let y = host.step(x);
            host.take_dte();
            count += 1;
            if count % 2 == 1 {
                to_encoder.push_back(y);
            }
        }
        let level = to_encoder.pop_front().unwrap_or(0.0);
        for x in net.down(level) {
            sent.push(caller.step(x));
            caller.take_dte();
            if caller.state() == State::Data && host.state() == State::Data {
                data_for += 1;
                if data_for == FS as usize {
                    let text: Vec<u8> = (0..200_000u32).map(|i| (i * 7 + 3) as u8).collect();
                    for &c in &text {
                        host.feed_dte(c);
                    }
                }
                if data_for > 2 * FS as usize {
                    take(&mut points, &caller);
                }
            }
        }
        if points.len() > 16_000 {
            break;
        }
    }
    if points.is_empty() {
        eprintln!("  no PCM data: the caller is in {} ({})", caller.line_phase(), caller.standard());
        return None;
    }
    Some(Picture { points, title: scope(&caller), peak: caller.constellation_peak() })
}

fn varc_call(carrier: &str, snr_db: f64) -> Option<Picture> {
    let (mut caller, mut host) = (Modem::new(FS), Modem::new(FS));
    type_at(&mut caller, &format!("AT+MS={carrier}"));
    type_at(&mut host, &format!("AT+MS={carrier}"));
    type_at(&mut host, "ATA");
    type_at(&mut caller, "ATD1");
    let mut hiss = [Hiss::new(snr_db, 5), Hiss::new(snr_db, 6), Hiss::new(snr_db, 7), Hiss::new(snr_db, 8)];
    let (mut from_caller, mut from_host) = ([0.0; 2], [0.0; 2]);
    let mut step = |caller: &mut Modem, host: &mut Modem| {
        let to_caller = [hiss[0].add(from_host[0]), hiss[1].add(from_host[1])];
        let to_host = [hiss[2].add(from_caller[0]), hiss[3].add(from_caller[1])];
        caller.wide_step(FS, &to_caller, &mut from_caller);
        host.wide_step(FS, &to_host, &mut from_host);
        caller.take_dte();
        host.take_dte();
    };
    let connected = (0..(40.0 * FS) as usize).any(|_| {
        step(&mut caller, &mut host);
        caller.state() == State::Data && host.state() == State::Data
    });
    if !connected {
        eprintln!("  no connection: the caller is in {}", caller.line_phase());
        return None;
    }
    for _ in 0..(1.0 * FS) as usize {
        step(&mut caller, &mut host);
    }
    caller.varc_points();
    let mut points = Vec::new();
    for _ in 0..(2.0 * FS) as usize {
        step(&mut caller, &mut host);
        points.extend(caller.varc_points());
    }
    Some(Picture { points, title: scope(&caller), peak: 1.0 })
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "docs/img/constellations".into());
    let only = args.get(2).cloned().unwrap_or_default();
    for (order, mode) in MODES.iter().enumerate().filter(|(_, m)| m.slug.contains(only.as_str())) {
        eprintln!("{}", mode.slug);
        let picture = match mode.call {
            Call::Line(ms) => line_call(ms, mode.end, mode.snr_db),
            Call::Fax(modulation) => fax_call(modulation, mode.snr_db),
            Call::Pcm => pcm_call(),
            Call::Varc(carrier) => varc_call(carrier, mode.snr_db),
        };
        let Some(picture) = picture else { continue };
        let noise = match mode.call {
            Call::Pcm => "G.711 network with noise".to_owned(),
            _ => format!("{} dB SNR", mode.snr_db),
        };
        eprintln!("  {}, {} points", picture.title, picture.points.len());
        let dir = format!("{out}/{}", mode.slug);
        std::fs::create_dir_all(&dir).expect("cannot make the folder");
        let body: String = picture.points.iter().map(|(x, y)| format!("{x:.5} {y:.5}\n")).collect();
        std::fs::write(format!("{dir}/points.txt"), body).expect("cannot write the points");
        std::fs::write(format!("{dir}/label.txt"), format!("{}\n{noise}\n{}\n{order}\n", picture.title, picture.peak))
            .expect("cannot write the label");
    }
}
