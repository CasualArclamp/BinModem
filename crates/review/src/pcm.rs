//! Phases 3 and 4 of a V.90 call, overheard on a recording with an end a
//! channel.
//!
//! The two directions are different modems here. The analogue modem's is
//! V.34's modulation, and is read with V.34's receiver the way the digital
//! modem reads it: S and S-bar, PP and TRN, then Ja with the description of
//! the DIL it wants, and in phase 4 its CPt and CP. The digital modem's is
//! PCM, read the way the analogue modem reads it: Sd, TRN1d, Jd, and the DIL
//! itself.
//!
//! Each needs something the other said -- the DIL is the one Ja asked for,
//! CPt comes at the size Jd named -- and each said it before the other
//! needed it. So the two channels are fed in step, a sample of each at a
//! time, and what one has read is there for the other when its turn comes.

use datapump::v32::{Mode, Scrambler};
use datapump::v34::dpsk::Side;
use datapump::v34::qam::Band;
use datapump::v34::receiver::{Heard as UpHeard, Receiver as V34Rx, Reference};
use datapump::v34::signals::{self, Reader, Size};
use datapump::v90::dil::Analysis;
use datapump::v90::pcm::{Heard as DownHeard, Receiver as PcmRx, Slicer};
use datapump::v90::sequences::{self, Cp, CpFinder, Descriptor, DescriptorFinder, JD_BITS, JD_PRIME_BITS, Jd};
use datapump::v90::ucode::{self, Law};
use dsp::Resampler;

use crate::model::{End, Event, Layer};
use crate::phase2::Settled;

/// The rate the PCM receiver has been run at and proved at.
const PCM_FS: f64 = 16_000.0;

/// What each direction has read that the other is waiting for.
#[derive(Debug, Default)]
struct Shared {
    descriptor: Option<Descriptor>,
    jd: Option<Jd>,
    /// When the analogue modem's second S-bar ended the DIL, in seconds.
    dil_over: Option<f64>,
    /// When the digital modem's S-bar-d went by: the analogue modem stops
    /// sending Ja on hearing it.
    s_bar_d: Option<f64>,
    /// When the analogue modem's E went by: both directions are in data
    /// mode within a few frames of it.
    e: Option<f64>,
    /// The downstream rate CP asked for.
    downstream_bps: Option<u32>,
}

/// What was learned, for the verdict.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub downstream_bps: Option<u32>,
    pub upstream_rates: Option<u16>,
    pub reached_data: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpStage {
    /// Before the first S-bar, or after an MD.
    AwaitS,
    Training,
    ReadJa,
    /// After Ja: the S-bar that answers J'd, and then the one that ends the
    /// DIL. Counts the ones heard.
    AfterJa(u8),
    Phase4,
    Data,
}

/// The analogue modem's transmission.
struct Upstream {
    fs: f64,
    who: End,
    band: Band,
    rx: V34Rx,
    reader: Reader,
    ja: DescriptorFinder,
    cps: CpFinder,
    stage: UpStage,
    /// When the S now on the line began.
    s_from: Option<f64>,
    trn_from: f64,
    trn_symbols: usize,
    in_trn: bool,
    ja_from: f64,
    ja_last: f64,
    ja_times: usize,
    trained_db: f64,
    /// Line power, smoothed, and what it was while Ja was going out.
    power: f64,
    alpha: f64,
    loud: f64,
    quiet: usize,
    /// When the line last came up out of silence.
    since: Option<f64>,
    ones: usize,
    size: Size,
    /// The CP now being repeated: its event's place in the list.
    cp_run: Option<(Cp, usize)>,
    last_cp: Option<Cp>,
    /// The points of whatever is being received now, for its event.
    seen: Vec<(f32, f32)>,
    events: Vec<Event>,
    outcome: Outcome,
}

/// Symbols held for one event before the older ones are let go.
const HELD: usize = 200_000;

impl Upstream {
    fn new(band: Band, who: End, fs: f64) -> Self {
        let mut rx = V34Rx::new(band, fs);
        rx.hunt();
        Self {
            fs,
            who,
            band,
            rx,
            reader: Reader::new(Mode::Answer),
            ja: DescriptorFinder::default(),
            cps: CpFinder::default(),
            stage: UpStage::AwaitS,
            s_from: None,
            trn_from: 0.0,
            trn_symbols: 0,
            in_trn: true,
            ja_from: 0.0,
            ja_last: 0.0,
            ja_times: 0,
            trained_db: 0.0,
            power: 0.0,
            alpha: 1.0 - (-1.0 / (0.005 * fs)).exp(),
            loud: 0.0,
            quiet: 0,
            since: None,
            ones: 0,
            size: Size::Four,
            cp_run: None,
            last_cp: None,
            seen: Vec::new(),
            events: Vec::new(),
            outcome: Outcome::default(),
        }
    }

    fn event(&self, start: f64, end: f64, layer: Layer, tag: &str) -> Event {
        let (carrier, half) = (self.band.carrier(), self.band.baud() / 2.0);
        Event::new(start, end, self.who, layer, tag).band((carrier - half).max(0.0), carrier + half)
    }

    fn feed(&mut self, n: usize, x: f64, shared: &mut Shared) {
        let now = n as f64 / self.fs;
        self.power += self.alpha * (x * x - self.power);
        if self.power.sqrt() > 0.003 {
            if self.since.is_none() {
                self.since = Some(now);
            }
        } else {
            self.since = None;
        }
        self.rx.feed(x);
        while let Some(heard) = self.rx.heard() {
            match heard {
                UpHeard::S => {
                    if self.s_from.is_none() {
                        self.s_from = Some(now);
                    }
                }
                UpHeard::Reversal { at } => self.reversal(now, at, shared),
                UpHeard::Trained { snr_db } => {
                    if self.stage == UpStage::Training {
                        self.trained_db = snr_db;
                        self.stage = UpStage::ReadJa;
                        self.in_trn = true;
                        self.trn_symbols = 0;
                        self.loud = self.power;
                        let pp = signals::PP_SYMBOLS as f64 / self.band.baud();
                        let from = self.trn_from;
                        self.events.push(
                            self.event(from, from + pp, Layer::Phase3, "PP")
                                .summary("PP: the sequence the far equaliser is first fitted to".to_owned()),
                        );
                        self.trn_from = from + pp;
                    }
                }
                UpHeard::Untrained => {
                    // An S and S-bar with no PP after them: the pair that
                    // comes before MD, most likely. The next pair is the one.
                    if self.stage == UpStage::Training {
                        self.stage = UpStage::AwaitS;
                        self.rx.hunt();
                    }
                }
                UpHeard::Symbol(symbol) => {
                    if self.seen.len() < HELD {
                        self.seen.push((symbol.point.re as f32, symbol.point.im as f32));
                    }
                    self.symbol(now, symbol.decided, shared);
                }
            }
        }
        // Ja goes on until the analogue modem hears S-bar-d and stops.
        if self.stage == UpStage::ReadJa && !self.in_trn {
            if self.power < 0.03 * self.loud {
                self.quiet += 1;
                if self.quiet as f64 > 0.01 * self.fs {
                    self.end_ja(now - 0.01, shared);
                }
            } else {
                self.quiet = 0;
                // Not every modem goes quiet. Half a second after S-bar-d
                // is longer than any round trip, and whatever is on the
                // line by then is not Ja.
                if shared.s_bar_d.is_some_and(|at| now > at + 0.5) {
                    self.end_ja(shared.s_bar_d.unwrap_or(now), shared);
                }
            }
        }
    }

    fn reversal(&mut self, now: f64, at: u64, shared: &mut Shared) {
        let baud = self.band.baud();
        // `at` is in half symbols, and so is the receiver's own count.
        let when = now - self.rx.halves().saturating_sub(at) as f64 / (2.0 * baud);
        let bar = signals::S_BAR_SYMBOLS as f64 / baud;
        let heard = self.s_from.take().unwrap_or(when - signals::S_SYMBOLS as f64 / baud);
        // The receiver hunts in whatever is on the line, the other end's
        // echo included: an S cannot have begun before the line came up.
        let from = heard.max(self.since.unwrap_or(heard)).min(when);
        // S lasts 128 symbols at the least. A turn with no S before it is
        // the tail of something else: the end of INFO1a, as often as not.
        if when - from < 0.01 && self.stage == UpStage::AwaitS {
            self.rx.hunt();
            return;
        }
        let layer = if matches!(self.stage, UpStage::AfterJa(1)) { Layer::Phase4 } else { Layer::Phase3 };
        self.seen.clear();
        self.events.push(self.event(from, when, layer, "S").summary(format!("S for {:.0} ms", 1000.0 * (when - from))));
        self.events.push(self.event(when, when + bar, layer, "S-bar").summary("S-bar: S turned over, 16 symbols".to_owned()));
        match self.stage {
            UpStage::AwaitS => {
                self.rx.train(Reference::PpThenTrn, Mode::Answer, at);
                self.stage = UpStage::Training;
                self.trn_from = when + bar;
            }
            UpStage::AfterJa(0) if shared.descriptor.as_ref().is_some_and(|d| !d.is_empty()) => {
                // The one that answers J'd. The DIL is going out; the one
                // that ends it is still to come.
                self.stage = UpStage::AfterJa(1);
                self.rx.hunt();
            }
            UpStage::AfterJa(_) => {
                shared.dil_over = Some(when);
                // CPt follows the S-bar straight away, read with the
                // equaliser phase 3 left.
                self.size = if shared.jd.is_some_and(|jd| jd.sixteen_in_training) { Size::Sixteen } else { Size::Four };
                self.rx.resume(at + 2 * signals::S_BAR_SYMBOLS as u64);
                self.rx.set_size(self.size);
                self.cps = CpFinder::default();
                self.ones = 0;
                self.stage = UpStage::Phase4;
            }
            _ => {}
        }
    }

    fn symbol(&mut self, now: f64, decided: datapump::v34::constellation::Point, shared: &mut Shared) {
        match self.stage {
            UpStage::ReadJa => {
                if self.in_trn {
                    let before = self.reader.clone();
                    let bits = self.reader.trn(decided, Size::Four);
                    self.trn_symbols += 1;
                    // The descrambler fills on TRN's first symbols.
                    if bits.iter().all(|b| *b) || self.trn_symbols < 24 {
                        return;
                    }
                    self.reader = before;
                    self.in_trn = false;
                    self.ja_from = now;
                    let from = self.trn_from;
                    let points = std::mem::take(&mut self.seen);
                    self.events.push(
                        self.event(from, now, Layer::Phase3, "TRN")
                            .qam(&points, 4)
                            .summary(format!(
                                "TRN, 4-point, {:.0} ms: trained a receiver to {:.0} dB",
                                1000.0 * (now - from),
                                self.trained_db
                            ))
                            .field("symbol rate", format!("{:.0} symbols/s", self.band.baud()))
                            .field("carrier", format!("{:.0} Hz", self.band.carrier()))
                            .field("signal to noise after training", format!("{:.1} dB", self.trained_db)),
                    );
                }
                for bit in self.reader.differential(decided, Size::Four) {
                    if let Some(descriptor) = self.ja.feed(bit) {
                        self.ja_times += 1;
                        self.ja_last = now;
                        if shared.descriptor.is_none() {
                            shared.descriptor = Some(descriptor);
                        }
                    }
                }
            }
            UpStage::Phase4 => {
                for bit in self.reader.differential(decided, self.size) {
                    self.ones = if bit { self.ones + 1 } else { 0 };
                    if self.ones >= signals::E_BITS && self.last_cp.as_ref().is_some_and(|cp| cp.data_mode) {
                        self.events.push(
                            self.event(now - signals::E_BITS as f64 / self.band.baud() / 2.0, now, Layer::Phase4, "E")
                                .summary("E: twenty ones, and data mode follows".to_owned()),
                        );
                        self.cp_run = None;
                        self.stage = UpStage::Data;
                        self.outcome.reached_data = true;
                        self.seen.clear();
                        shared.e = Some(now);
                        shared.downstream_bps = self.outcome.downstream_bps;
                        self.events.push(
                            self.event(now, now, Layer::Data, "data")
                                .summary("data mode, V.34 modulation from the analogue modem".to_owned()),
                        );
                        return;
                    }
                    if let Some(cp) = self.cps.feed(bit) {
                        self.heard_cp(now, cp);
                    }
                }
            }
            UpStage::Data => {
                // The event runs for as long as symbols keep coming.
                if let Some(data) = self.events.iter_mut().rev().find(|e| e.tag == "data") {
                    data.end = now;
                }
            }
            _ => {}
        }
    }

    fn end_ja(&mut self, now: f64, shared: &mut Shared) {
        let points = std::mem::take(&mut self.seen);
        let mut event = self.event(self.ja_from, now, Layer::Phase3, "Ja").qam(&points, 4);
        event = match &shared.descriptor {
            Some(d) => event
                .summary(format!(
                    "Ja: asks for a DIL of {} segments, {} symbols ({:.2} s)",
                    d.ucodes.len(),
                    d.len(),
                    d.len() as f64 / 8000.0
                ))
                .fields(descriptor_fields(d))
                .field("read whole", format!("{} times", self.ja_times)),
            None => event.summary("Ja, but no DIL descriptor in it passed its check".to_owned()),
        };
        self.events.push(event);
        self.stage = UpStage::AfterJa(0);
        self.s_from = None;
        self.rx.hunt();
    }

    fn heard_cp(&mut self, now: f64, cp: Cp) {
        self.last_cp = Some(cp.clone());
        if let Some((was, index)) = &self.cp_run
            && *was == cp
        {
            let event = &mut self.events[*index];
            event.end = now;
            let more = std::mem::take(&mut self.seen);
            if event.symbols.len() + more.len() <= crate::model::SYMBOLS_KEPT {
                event.symbols.extend(more);
            }
            let times = event.fields.iter_mut().find(|(name, _)| name == "sent");
            if let Some((_, value)) = times {
                let n: usize = value.split(' ').next().and_then(|v| v.parse().ok()).unwrap_or(1);
                *value = format!("{} times", n + 1);
            }
            return;
        }
        let tag = match (cp.data_mode, cp.acknowledge) {
            (false, _) => "CPt",
            (true, false) => "CP",
            (true, true) => "CP'",
        };
        let bits = cp.frame_bits() as f64 / (self.band.baud() * if self.size == Size::Sixteen { 4.0 } else { 2.0 });
        let rate = if cp.data_mode {
            sequences::data_rate(cp.drn)
        } else {
            Some(datapump::v90::rate_for(u32::from(cp.drn) + 8))
        };
        let summary = match (cp.data_mode, rate) {
            (true, Some(rate)) => {
                self.outcome.downstream_bps = Some(rate);
                self.outcome.upstream_rates = Some(cp.upstream_rates);
                format!("{tag}: {rate} bit/s downstream")
            }
            (true, None) if cp.drn == 0 => format!("{tag}: cleardown"),
            (false, Some(rate)) => format!("{tag}: training constellations, {rate} bit/s in phase 4"),
            _ => format!("{tag}: rate number {}", cp.drn),
        };
        let points = std::mem::take(&mut self.seen);
        let size = if self.size == Size::Sixteen { 16 } else { 4 };
        let event = self
            .event(now - bits, now, Layer::Phase4, tag)
            .summary(summary)
            .fields(cp_fields(&cp))
            .field("sent", "1 times")
            .qam(&points, size);
        self.events.push(event);
        self.cp_run = Some((cp, self.events.len() - 1));
    }
}

fn descriptor_fields(d: &Descriptor) -> Vec<(String, String)> {
    let run = |list: &[u8]| list.iter().map(u8::to_string).collect::<Vec<_>>().join(" ");
    let bits = |list: &[bool]| list.iter().map(|b| if *b { '1' } else { '0' }).collect::<String>();
    let training: Vec<u8> = d.ucodes.clone();
    let (low, high) = (training.iter().min().copied().unwrap_or(0), training.iter().max().copied().unwrap_or(0));
    vec![
        ("segments".to_owned(), d.ucodes.len().to_string()),
        ("length".to_owned(), format!("{} symbols, {:.2} s", d.len(), d.len() as f64 / 8000.0)),
        ("Ucodes asked for".to_owned(), format!("{low} to {high}")),
        ("segment lengths H".to_owned(), run(&d.h)),
        ("reference Ucodes".to_owned(), run(&d.refs)),
        ("sign pattern".to_owned(), format!("{} bits: {}", d.signs.len(), bits(&d.signs))),
        ("training pattern".to_owned(), format!("{} bits: {}", d.training.len(), bits(&d.training))),
        ("Ucodes in order".to_owned(), run(&d.ucodes)),
    ]
}

fn cp_fields(cp: &Cp) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut add = |name: &str, value: String| fields.push((name.to_owned(), value));
    add("kind", if cp.data_mode { "CP: data mode" } else { "CPt: phase 4 training" }.to_owned());
    add("rate number", cp.drn.to_string());
    if cp.data_mode
        && let Some(rate) = sequences::data_rate(cp.drn)
    {
        add("downstream rate", format!("{rate} bit/s"));
    }
    add("acknowledges MP", if cp.acknowledge { "yes" } else { "no" }.to_owned());
    add("codec", if cp.a_law { "A-law" } else { "mu-law" }.to_owned());
    add("sign redundancy Sr", format!("{:?}", cp.redundancy));
    add("shaper look-ahead", cp.lookahead.to_string());
    add("silence asked for", if cp.silence { "yes" } else { "no" }.to_owned());
    let ups: Vec<String> =
        (0..13).filter(|bit| cp.upstream_rates >> bit & 1 == 1).map(|bit| (4800 + 2400 * bit).to_string()).collect();
    add("upstream rates enabled", if ups.is_empty() { "none".to_owned() } else { ups.join(", ") });
    add("TRN1d level ratio", format!("{:.3}", f64::from(cp.trn1d_ratio) / 8192.0));
    add(
        "shaping filter",
        format!(
            "a1 {:+.3}, a2 {:+.3}, b1 {:+.3}, b2 {:+.3}",
            f64::from(cp.shaping[0]) / 64.0,
            f64::from(cp.shaping[1]) / 64.0,
            f64::from(cp.shaping[2]) / 64.0,
            f64::from(cp.shaping[3]) / 64.0
        ),
    );
    let sizes: Vec<String> = (0..6).map(|i| cp.points(i).len().to_string()).collect();
    add("points in each interval", sizes.join(" "));
    add("constellation of each interval", cp.intervals.iter().map(u8::to_string).collect::<Vec<_>>().join(" "));
    add("constellations", cp.constellations.len().to_string());
    for i in 0..6 {
        let points = cp.points(i);
        add(&format!("interval {i} Ucodes"), points.iter().map(u8::to_string).collect::<Vec<_>>().join(" "));
    }
    add("codec constellations given", if cp.codec.is_some() { "yes" } else { "no" }.to_owned());
    fields
}

/// A DIL under way: the symbol it began at, when, and the codeword and
/// sign of each symbol of it.
type Dil = (u64, f64, Vec<(u8, bool)>);

/// The digital modem's transmission.
struct Downstream {
    who: End,
    law: Law,
    rx: PcmRx,
    up: Option<Resampler>,
    scratch: Vec<f64>,
    descrambler: Scrambler,
    differential: bool,
    previous: bool,
    bits: Vec<bool>,
    /// Line power, and when it last came up out of silence.
    power: f64,
    alpha: f64,
    since: Option<f64>,
    reversal: Option<f64>,
    trained: Option<(f64, bool)>,
    trn_over: Option<f64>,
    /// The last Jd read: the symbol after it, and when.
    jd: Option<(u64, f64, Jd)>,
    jd_first: Option<f64>,
    jd_times: usize,
    /// The DIL: the symbol it began at, when, and what it is made of.
    dil: Option<Dil>,
    analysis: Analysis,
    dil_read: usize,
    /// When the DIL ended, once it has; and whether phase 4 has been cut
    /// from data mode since.
    after_dil: Option<f64>,
    in_data: Option<f64>,
    /// The levels of whatever is being received now, for its event.
    seen: Vec<f32>,
    events: Vec<Event>,
}

impl Downstream {
    fn new(law: Law, uinfo: u8, who: End, fs: f64) -> Self {
        let mut rx = PcmRx::new(law, PCM_FS);
        rx.hunt(uinfo);
        Self {
            who,
            law,
            rx,
            up: (fs != PCM_FS).then(|| Resampler::new(fs, PCM_FS)),
            scratch: Vec::new(),
            descrambler: Scrambler::new(Mode::Call),
            differential: false,
            previous: false,
            bits: Vec::new(),
            power: 0.0,
            alpha: 1.0 - (-1.0 / (0.005 * fs)).exp(),
            since: None,
            reversal: None,
            trained: None,
            trn_over: None,
            jd: None,
            jd_first: None,
            jd_times: 0,
            dil: None,
            analysis: Analysis::new(),
            dil_read: 0,
            after_dil: None,
            in_data: None,
            seen: Vec::new(),
            events: Vec::new(),
        }
    }

    fn event(&self, start: f64, end: f64, tag: &str) -> Event {
        Event::new(start, end, self.who, Layer::Phase3, tag)
    }

    fn feed(&mut self, now: f64, x: f64, shared: &mut Shared) {
        // Phase 4 gives way to data mode when the analogue modem's E has
        // gone by: Ed and B1d are within a few frames of it.
        if let (Some(from), Some(e), None) = (self.after_dil, shared.e, self.in_data) {
            let levels = std::mem::take(&mut self.seen);
            self.events.push(
                Event::new(from, e, self.who, Layer::Phase4, "phase 4")
                    .summary("Ri, TRN2d, MP and Ed from the digital modem: symbols only, not read".to_owned())
                    .pcm(&levels),
            );
            self.in_data = Some(e);
        }
        self.power += self.alpha * (x * x - self.power);
        if self.power.sqrt() > 0.004 {
            if self.since.is_none() {
                self.since = Some(now);
            }
        } else if self.reversal.is_none() {
            self.since = None;
        }
        // The DIL ends when the analogue modem says so.
        if let (Some(over), Some((_, from, _))) = (shared.dil_over, &self.dil)
            && now >= over
        {
            let from = *from;
            self.end_dil(from, now);
            return;
        }
        self.scratch.clear();
        match &mut self.up {
            Some(up) => up.process(x, &mut self.scratch),
            None => self.scratch.push(x),
        }
        for i in 0..self.scratch.len() {
            let sample = self.scratch[i];
            self.rx.feed(sample);
            while let Some(heard) = self.rx.heard() {
                self.heard(now, heard, shared);
            }
        }
    }

    fn heard(&mut self, now: f64, heard: DownHeard, shared: &mut Shared) {
        match heard {
            DownHeard::Reversal { .. } => {
                if self.reversal.is_none() {
                    self.reversal = Some(now);
                    shared.s_bar_d = Some(now);
                    let from = self.since.unwrap_or(now - 0.02).min(now);
                    self.events.push(
                        self.event(from, now, "Sd").summary(format!("Sd for {:.0} ms: the digital modem's S", 1000.0 * (now - from))),
                    );
                    self.events
                        .push(self.event(now, now + 0.001, "S-bar-d").summary("S-bar-d: Sd turned over, 8 symbols".to_owned()));
                }
            }
            DownHeard::Trained { snr_db, inverted } => self.trained = Some((snr_db, inverted)),
            DownHeard::Symbol(s) => {
                if self.seen.len() < HELD {
                    self.seen.push(s.value as f32);
                }
                if self.after_dil.is_some() {
                    return;
                }
                if let Some((first, _, levels)) = &self.dil {
                    // What was sent at this place in the DIL, which goes
                    // round again if the analogue modem lets it run on.
                    let place = (s.index.saturating_sub(*first)) as usize;
                    if !levels.is_empty() {
                        let (ucode, positive) = levels[place % levels.len()];
                        self.analysis.feed(ucode, positive, s.interval(), s.value);
                        self.dil_read += 1;
                    }
                    return;
                }
                let sign = s.positive();
                let before = self.descrambler.clone();
                let mut bit = self.descrambler.descramble(if self.differential { sign ^ self.previous } else { sign });
                if !self.differential && !bit {
                    // TRN1d is over: Jd is differential from this symbol on.
                    self.differential = true;
                    self.descrambler = before;
                    bit = self.descrambler.descramble(sign ^ self.previous);
                    self.trn_over = Some(now);
                }
                self.previous = sign;
                self.bits.push(bit);
                if self.bits.len() >= JD_BITS
                    && let Some(jd) = Jd::from_bits(&self.bits[self.bits.len() - JD_BITS..])
                {
                    self.jd_times += 1;
                    if self.jd_first.is_none() {
                        let first = now - JD_BITS as f64 / 8000.0;
                        self.jd_first = Some(first);
                        // TRN1d ran from S-bar-d to here, whatever a bad
                        // bit early in it looked like.
                        let from = self.reversal.map_or(first, |r| r + 0.001);
                        let (snr, inverted) = self.trained.unwrap_or((0.0, false));
                        // All but the Jd that has just been read.
                        let keep = self.seen.len().saturating_sub(JD_BITS);
                        let levels: Vec<f32> = self.seen.drain(..keep).collect();
                        self.events.push(
                            self.event(from, first, "TRN1d")
                                .pcm(&levels)
                                .summary(format!("TRN1d, {:.2} s: trained a receiver to {snr:.0} dB", first - from))
                                .field("signal to noise after training", format!("{snr:.1} dB"))
                                .field("signal turned over on the way", if inverted { "yes" } else { "no" })
                                .field("coding", if self.law == Law::A { "A-law" } else { "mu-law" }),
                        );
                    }
                    shared.jd = Some(jd);
                    self.jd = Some((s.index + 1, now, jd));
                }
                // J'd: twelve zeros where the next Jd's sync would be, and
                // the DIL straight after.
                if let Some((end, at, jd)) = self.jd
                    && s.index + 1 == end + JD_PRIME_BITS as u64
                    && self.bits[self.bits.len() - JD_PRIME_BITS..].iter().all(|b| !*b)
                {
                    let first = self.jd_first.unwrap_or(at);
                    let levels = std::mem::take(&mut self.seen);
                    self.events.push(
                        self.event(first, at, "Jd")
                            .pcm(&levels)
                            .summary(format!(
                                "Jd: {} of 22 rates enabled, {}-point training",
                                (1..=22u8).filter(|&drn| jd.enables(drn)).count(),
                                if jd.sixteen_in_training { 16 } else { 4 }
                            ))
                            .fields(jd_fields(&jd))
                            .field("sent", format!("{} times", self.jd_times)),
                    );
                    self.events.push(self.event(at, now, "J'd").summary("J'd: twelve zeros, and the DIL follows".to_owned()));
                    let levels: Vec<(u8, bool)> = shared.descriptor.as_ref().map(|d| d.symbols().collect()).unwrap_or_default();
                    if !levels.is_empty() {
                        let law = self.law;
                        self.rx.expect(
                            levels.iter().map(move |&(u, positive)| ucode::level(law, u) * if positive { 1.0 } else { -1.0 }),
                        );
                    }
                    self.dil = Some((s.index + 1, now, levels));
                }
            }
            _ => {}
        }
    }

    /// What is left when the recording ends.
    fn finish(&mut self, now: f64, shared: &Shared) {
        let levels = std::mem::take(&mut self.seen);
        match (self.in_data, self.after_dil) {
            (Some(from), _) => {
                let rate = shared.downstream_bps.map_or_else(String::new, |r| format!(" at {r} bit/s"));
                self.events.push(
                    Event::new(from, now, self.who, Layer::Data, "data")
                        .summary(format!("data mode, PCM{rate}: symbols only, not read"))
                        .pcm(&levels),
                );
            }
            (None, Some(from)) => self.events.push(
                Event::new(from, now, self.who, Layer::Phase4, "phase 4")
                    .summary("Ri, TRN2d, MP and Ed from the digital modem: symbols only, not read".to_owned())
                    .pcm(&levels),
            ),
            _ => {}
        }
    }

    fn end_dil(&mut self, from: f64, now: f64) {
        self.after_dil = Some(now);
        self.dil = None;
        // Nothing is known of what comes next, so nothing is decided: the
        // equaliser is left as the DIL had it.
        self.rx.set_slicer(Slicer::Free);
        let route = self.analysis.route();
        let noise = route.noise();
        // The reference level is what the DIL's loudest codewords come to,
        // which is near enough the power data mode is sent at.
        let levels = std::mem::take(&mut self.seen);
        let mut event = self
            .event(from, now, "DIL")
            .pcm(&levels)
            .summary(format!("DIL for {:.2} s: {} symbols read", now - from, self.dil_read))
            .field("symbols read", self.dil_read.to_string());
        if self.dil_read > 0 && noise > 0.0 {
            event = event.field("spread of a codeword", format!("{:.1} of the 16-bit scale", noise * 32768.0));
            // How far each interval's levels are from the mean of the six:
            // a robbed bit or a pad that is not the same in every frame
            // shows as one or two intervals standing apart.
            let mut gains = Vec::new();
            for interval in &route.levels {
                let ratios: Vec<f64> = interval
                    .iter()
                    .enumerate()
                    .filter(|(u, level)| *u >= 40 && **level > 0.0)
                    .map(|(u, level)| level / ucode::level(self.law, u as u8))
                    .collect();
                if !ratios.is_empty() {
                    gains.push(20.0 * (ratios.iter().sum::<f64>() / ratios.len() as f64).log10());
                }
            }
            if gains.len() == 6 {
                event = event.field(
                    "level in each interval",
                    gains.iter().map(|g| format!("{g:+.2}")).collect::<Vec<_>>().join(" ") + " dB",
                );
            }
        }
        self.events.push(event);
    }
}

fn jd_fields(jd: &Jd) -> Vec<(String, String)> {
    let rates: Vec<u32> = (1..=22u8).filter(|&drn| jd.enables(drn)).filter_map(sequences::data_rate).collect();
    vec![
        (
            "downstream rates enabled".to_owned(),
            match (rates.first(), rates.last()) {
                (Some(low), Some(high)) => format!("{} of 22, {low} to {high} bit/s", rates.len()),
                _ => "none".to_owned(),
            },
        ),
        ("phase 4 training".to_owned(), format!("{}-point", if jd.sixteen_in_training { 16 } else { 4 })),
        ("renegotiation".to_owned(), format!("{}-point", if jd.sixteen_in_renegotiation { 16 } else { 4 })),
        ("shaper look-ahead it can do".to_owned(), jd.lookahead.to_string()),
    ]
}

/// Follow a V.90 call from the end of phase 2. `analogue` and `digital` are
/// the two modems' own transmissions.
pub fn follow(
    analogue: (&[f32], End),
    digital: (&[f32], End),
    fs: f64,
    settled: &Settled,
    until: f64,
) -> Option<(Vec<Event>, Outcome)> {
    let asked = settled.info1a_pcm?;
    let from = settled.ended?;
    // INFO1d says which carrier the upstream goes on at each rate; a short
    // phase 2 has no INFO1d, and its INFO1a says instead.
    let high = match (settled.upstream_high_carrier, &settled.info1c) {
        (Some(high), _) => high,
        (None, Some(c)) => c.probed[asked.upstream.index() as usize].high_carrier,
        (None, None) => false,
    };
    let law = if settled.info0d.is_some_and(|d| d.a_law) { Law::A } else { Law::Mu };
    let mut shared = Shared::default();
    let mut up = Upstream::new(Band::new(asked.upstream, high), analogue.1, fs);
    let mut down = Downstream::new(law, asked.uinfo, digital.1, fs);
    let first = ((from + 0.01) * fs) as usize;
    let last = analogue.0.len().min(digital.0.len()).min((until * fs) as usize);
    for n in first..last {
        up.feed(n, analogue.0[n] as f64, &mut shared);
        down.feed(n as f64 / fs, digital.0[n] as f64, &mut shared);
    }
    let _ = Side::Call;
    let end = last as f64 / fs;
    down.finish(end, &shared);
    if let Some(data) = up.events.iter_mut().rev().find(|e| e.tag == "data") {
        let seen = std::mem::take(&mut up.seen);
        *data = data.clone().qam(&seen, 1024);
    }
    let mut events = up.events;
    events.extend(down.events);
    Some((events, up.outcome))
}
