//! Phase 2 of V.34, V.90 and V.92, overheard: the INFO sequences, tones A
//! and B and their reversals, and the line probing.
//!
//! The two ends are on a carrier each here, 1200 Hz for the calling modem
//! and 2400 Hz for the answering one, so a tap that has both reads both. On
//! a V.90 or V.92 call the digital modem takes the calling modem's part and
//! the analogue one the answering modem's, whichever of them dialled.

use datapump::v34::dpsk::{Receiver as InfoRx, Side};
use datapump::v34::info::{self, Info, Info0, Info0d, Info1a, Info1aPcm, Info1aPcmUp, Info1c, Probed, SymbolRate};
use datapump::v34::probe::{Analyzer, Reading};
use dsp::{ReversalDetector, ToneDetector};

use crate::model::{End, Event, Layer};
use crate::setup::db;

/// What phase 2 settled, for whoever follows the call past it.
#[derive(Debug, Clone, Default)]
pub struct Settled {
    pub info0_call: Option<Info0>,
    pub info0_answer: Option<Info0>,
    pub info0d: Option<Info0d>,
    pub info1c: Option<Info1c>,
    pub info1a: Option<Info1a>,
    pub info1a_pcm: Option<Info1aPcm>,
    pub info1a_pcm_up: Option<Info1aPcmUp>,
    /// The carrier a short phase 2's INFO1a put the upstream on, which is
    /// where a full one's INFO1d would have said.
    pub upstream_high_carrier: Option<bool>,
    /// When the last INFO1 ended, in seconds: phase 3 starts after it.
    pub ended: Option<f64>,
    /// When the first INFO sequence of this phase 2 began.
    pub began: Option<f64>,
}

impl Settled {
    pub fn merge(&mut self, other: Settled) {
        macro_rules! take {
            ($($field:ident),*) => { $( if self.$field.is_none() { self.$field = other.$field; } )* };
        }
        take!(info0_call, info0_answer, info0d, info1c, info1a, info1a_pcm, info1a_pcm_up, upstream_high_carrier);
        self.ended = match (self.ended, other.ended) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
        self.began = match (self.began, other.began) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
}

fn yes(flag: bool) -> String {
    if flag { "yes" } else { "no" }.to_owned()
}

fn rate(rate: SymbolRate) -> String {
    format!("{} symbols/s", rate.nominal())
}

fn info0_fields(i: &Info0, pcm: bool) -> Vec<(String, String)> {
    let mut rates = vec!["2400", "3000", "3200"];
    if i.rate_2743 {
        rates.push("2743");
    }
    if i.rate_2800 {
        rates.push("2800");
    }
    if i.rate_3429 {
        rates.push("3429");
    }
    rates.sort_unstable();
    let carriers = |low: bool, high: bool| match (low, high) {
        (true, true) => "low and high carrier",
        (true, false) => "low carrier only",
        (false, true) => "high carrier only",
        (false, false) => "neither carrier",
    };
    let mut fields = vec![
        ("symbol rates".to_owned(), rates.join(", ")),
        ("at 3000".to_owned(), carriers(i.low_carrier_3000, i.high_carrier_3000).to_owned()),
        ("at 3200".to_owned(), carriers(i.low_carrier_3200, i.high_carrier_3200).to_owned()),
        ("may transmit at 3429".to_owned(), yes(i.transmit_3429)),
        ("can reduce power".to_owned(), yes(i.can_reduce_power)),
        ("rate asymmetry allowed".to_owned(), format!("{} steps", i.asymmetry)),
        ("1664-point constellations".to_owned(), yes(i.constellation_1664)),
    ];
    if pcm {
        // V.92 gives the two clock bits to itself (Tables 15 and 16/V.92).
        fields.push(("V.92 capable".to_owned(), yes(i.clock & 1 != 0)));
        fields.push(("asks for short phase 2".to_owned(), yes(i.clock & 2 != 0)));
    } else {
        fields.push(("transmit clock".to_owned(), match i.clock {
            0 => "internal".to_owned(),
            1 => "locked to the receive timing".to_owned(),
            2 => "external".to_owned(),
            n => format!("reserved ({n})"),
        }));
    }
    fields.push(("acknowledges an INFO0".to_owned(), yes(i.acknowledge)));
    fields
}

fn probed_fields(p: &Probed, at: &str) -> Vec<(String, String)> {
    vec![(
        at.to_owned(),
        format!(
            "{} carrier, pre-emphasis {}, up to {} bit/s",
            if p.high_carrier { "high" } else { "low" },
            p.pre_emphasis,
            u32::from(p.max_rate) * 2400
        ),
    )]
}

/// The event for one INFO sequence heard to end at `end`.
fn info_event(info: &Info, side: Side, who: End, end: f64, pcm_call: bool, settled: &mut Settled) -> Event {
    let carrier = side.carrier();
    let make = |tag: &str, bits: usize| {
        Event::new((end - bits as f64 / 600.0).max(0.0), end, who, Layer::Phase2, tag)
            .band(carrier - 330.0, carrier + 330.0)
            .field("carrier", format!("{carrier:.0} Hz, 600 bit/s DPSK"))
    };
    match info {
        Info::Info0(i) => {
            let (tag, slot) = if side == Side::Call {
                ("INFO0c", &mut settled.info0_call)
            } else {
                ("INFO0a", &mut settled.info0_answer)
            };
            *slot = Some(*i);
            // An answering modem's INFO0 on a PCM call is INFO0a of V.90,
            // whose clock bits V.92 took.
            let pcm = side == Side::Answer && pcm_call;
            let mut top = Vec::new();
            if i.rate_3429 {
                top.push("3429");
            }
            if i.constellation_1664 {
                top.push("1664 points");
            }
            make(tag, info::INFO0_BITS)
                .summary(format!("capabilities{}", if top.is_empty() { String::new() } else { format!(": {}", top.join(", ")) }))
                .fields(info0_fields(i, pcm))
        }
        Info::Info0d(d) => {
            settled.info0d = Some(*d);
            let flags = d.pcm_flags();
            make("INFO0d", info::INFO0D_BITS)
                .summary(format!(
                    "digital modem: {}, {}{}",
                    if d.a_law { "A-law" } else { "mu-law" },
                    if flags.v92 { "V.92" } else { "V.90" },
                    if flags.short_phase2 { ", asks for short phase 2" } else { "" }
                ))
                .field("coding", if d.a_law { "A-law" } else { "mu-law" })
                .field("V.92 capable", yes(flags.v92))
                .field("asks for short phase 2", yes(flags.short_phase2))
                .field("nominal transmit power", format!("{:.1} dBm0", d.nominal_dbm0()))
                .field("maximum transmit power", format!("{:.1} dBm0", d.max_dbm0()))
                .field("power measured", if d.power_at_codec { "at the codec output" } else { "at the modem terminals" })
                .field("upstream at 3429", yes(d.upstream_3429))
                .fields(info0_fields(&d.v34, false).into_iter().filter(|(n, _)| n != "transmit clock"))
        }
        Info::Info1c(c) => {
            settled.info1c = Some(*c);
            settled.ended = Some(end);
            let tag = if pcm_call { "INFO1d" } else { "INFO1c" };
            let mut event = make(tag, info::INFO1C_BITS)
                .summary("what the probing showed, at each symbol rate".to_owned())
                .field("minimum power reduction", format!("{} dB", c.min_power_reduction))
                .field("additional power reduction", format!("{} dB", c.additional_power_reduction))
                .field("MD length", format!("{} ms", u32::from(c.md_length) * 35));
            for (rate, probed) in SymbolRate::ALL.iter().zip(&c.probed) {
                event = event.fields(probed_fields(probed, &format!("at {}", rate.nominal())));
            }
            if let Some(offset) = c.frequency_offset {
                event = event.field("frequency offset", format!("{offset:+.2} Hz"));
            }
            if pcm_call {
                event = event.field("V.92 PCM upstream", yes(c.pcm_upstream()));
            }
            event
        }
        Info::Info1a(a) => {
            settled.info1a = Some(*a);
            settled.ended = Some(end);
            let mut event = make("INFO1a", info::INFO1A_BITS)
                .summary(format!(
                    "{} from the caller, {} from the answerer",
                    rate(a.call_to_answer),
                    rate(a.answer_to_call)
                ))
                .field("caller to answerer", rate(a.call_to_answer))
                .field("answerer to caller", rate(a.answer_to_call))
                .fields(probed_fields(&a.probed, "caller is to use"))
                .field("minimum power reduction", format!("{} dB", a.min_power_reduction))
                .field("additional power reduction", format!("{} dB", a.additional_power_reduction))
                .field("MD length", format!("{} ms", u32::from(a.md_length) * 35));
            if let Some(offset) = a.frequency_offset {
                event = event.field("frequency offset", format!("{offset:+.2} Hz"));
            }
            event
        }
        Info::Info1aPcm(a) => {
            settled.info1a_pcm = Some(*a);
            settled.ended = Some(end);
            pcm_info1a(make("INFO1a", info::INFO1A_BITS), a)
        }
        Info::Info1aV34Up(a) => {
            settled.info1a_pcm = Some(a.v90);
            settled.upstream_high_carrier = Some(a.high_carrier);
            settled.ended = Some(end);
            pcm_info1a(make("INFO1a", info::INFO1A_BITS), &a.v90)
                .field("upstream carrier", if a.high_carrier { "high" } else { "low" })
        }
        Info::Info1aPcmUp(a) => {
            settled.info1a_pcm_up = Some(*a);
            settled.ended = Some(end);
            make("INFO1a", info::INFO1A_BITS)
                .summary("V.92 PCM upstream chosen".to_owned())
                .field("upstream", "PCM, 8000 symbols/s")
                .field("precoder and prefilter sections", a.sections.to_string())
                .field("Ltot code", a.ltot_code.to_string())
                .field("Lmax code", a.lmax_code.to_string())
                .field("MD length", format!("{} ms", u32::from(a.md_length) * 35))
                .field("U_INFO", format!("Ucode {}", a.uinfo))
        }
        Info::InfoH(h) => make("INFOh", info::INFOH_BITS)
            .summary(format!("half-duplex: {}", rate(h.symbol_rate)))
            .field("symbol rate", rate(h.symbol_rate))
            .field("carrier chosen", if h.high_carrier { "high" } else { "low" })
            .field("pre-emphasis", h.pre_emphasis.to_string())
            .field("power reduction", format!("{} dB", h.power_reduction))
            .field("TRN length", format!("{} symbols", h.trn_symbols())),
        Info::Mh(m) => make("MH", info::MH_BITS)
            .summary(format!("modem on hold: {:?}", m.indication))
            .field("indication", format!("{:?}", m.indication))
            .field("information", format!("{:?}", m.information)),
    }
}

fn pcm_info1a(event: Event, a: &Info1aPcm) -> Event {
    let mut event = event
        .summary(format!("V.34 upstream at {}", rate(a.upstream)))
        .field("upstream symbol rate", rate(a.upstream))
        .field("U_INFO", format!("Ucode {}", a.uinfo))
        .field("MD length", format!("{} ms", u32::from(a.md_length) * 35));
    if let Some(offset) = a.frequency_offset {
        event = event.field("frequency offset", format!("{offset:+.2} Hz"));
    }
    event
}

/// The comb of the probing signals either side of a carrier, and the carrier
/// itself, which L1 and L2 leave out.
struct Probing {
    comb: Vec<ToneDetector>,
    holes: Vec<ToneDetector>,
}

impl Probing {
    fn new(fs: f64) -> Self {
        let tone = |hz: f64| ToneDetector::new(hz, 20.0, fs);
        Self {
            comb: [450.0, 750.0, 1050.0, 1350.0, 1650.0, 1950.0, 2250.0, 2550.0].into_iter().map(tone).collect(),
            holes: [900.0, 1200.0, 1800.0, 2400.0].into_iter().map(tone).collect(),
        }
    }

    fn feed(&mut self, x: f64) {
        for tone in self.comb.iter_mut().chain(self.holes.iter_mut()) {
            tone.feed(x);
        }
    }

    /// Whether what is on the line is a probing signal: the comb standing,
    /// and the gaps in it empty.
    fn standing(&self, rms: f64) -> bool {
        if rms < 1e-3 {
            return false;
        }
        // Twenty-one tones of equal level: each a fifth or so of the whole.
        let floor = 0.08 * rms;
        let teeth = self.comb.iter().filter(|t| t.amplitude() > floor).count();
        let mean = self.comb.iter().map(ToneDetector::amplitude).sum::<f64>() / self.comb.len() as f64;
        let hollow = self.holes.iter().filter(|t| t.amplitude() < 0.35 * mean).count();
        teeth >= 7 && hollow >= 3
    }
}

/// One end's phase 2, off the channel it is on.
pub fn listen(
    samples: &[f32],
    fs: f64,
    side: Side,
    who: End,
    pcm_call: bool,
    from: f64,
) -> (Vec<Event>, Vec<Settled>) {
    let mut events: Vec<Event> = Vec::new();
    let mut settled = Settled::default();
    // A call has a phase 2 for its start-up and another for every retrain.
    let mut rounds: Vec<Settled> = Vec::new();
    let carrier = side.carrier();
    let (tone_tag, tone_name) = if side == Side::Call { ("B", "tone B") } else { ("A", "tone A") };

    let mut plain = InfoRx::new(side, fs);
    // V.92's short phase 2 reads the answering modem's INFO1a differently,
    // and nothing in the sequence says which it is.
    let mut short = (side == Side::Answer).then(|| InfoRx::new(side, fs).in_short_phase2());
    let mut tone = ToneDetector::new(carrier, 40.0, fs);
    let mut reversals = ReversalDetector::new(carrier, 60.0, 0.004, fs);
    let mut probing = Probing::new(fs);
    let alpha = 1.0 - (-1.0 / (0.02 * fs)).exp();
    let mut power = 0.0f64;

    let first = (from * fs) as usize;
    let blink = (0.02 * fs) as usize;
    // As heard, before any of it is weighed against the rest: where the
    // carrier stood alone, where it turned, where the comb stood.
    let mut spans: Vec<(usize, usize, f64)> = Vec::new();
    let mut standing: Option<(usize, usize, f64)> = None;
    let mut turns: Vec<usize> = Vec::new();
    let mut combs: Vec<(usize, usize, f64)> = Vec::new();
    let mut comb: Option<(usize, usize, f64)> = None;
    let mut last_info: Option<(String, f64)> = None;
    // The carrier as the INFO receiver has it, a point a symbol; and as a
    // phasor, every few milliseconds, for the tones.
    let mut dpsk: Vec<(usize, (f32, f32))> = Vec::new();
    let mut last_point = None;
    let mut phasors: Vec<(usize, (f32, f32))> = Vec::new();
    let every = ((fs / 400.0) as usize).max(1);
    let points_between = |list: &[(usize, (f32, f32))], a: usize, b: usize| -> Vec<(f32, f32)> {
        list.iter().filter(|(n, _)| *n >= a && *n <= b).map(|(_, p)| *p).collect()
    };

    for (n, &sample) in samples.iter().enumerate().skip(first) {
        let x = sample as f64;
        let now = n as f64 / fs;
        power += alpha * (x * x - power);
        let rms = power.sqrt();

        let mut heard = plain.feed(x);
        let point = plain.scope_point();
        if point != last_point {
            last_point = point;
            if let Some((re, im)) = point {
                dpsk.push((n, (re as f32, im as f32)));
            }
        }
        if let Some(short) = &mut short {
            let other = short.feed(x);
            // The plain reading comes first; the short one only where the
            // far end asked for a short phase 2 and the plain made nothing.
            if heard.is_none() && settled.info0_answer.is_some_and(|i| i.clock & 2 != 0) {
                heard = other;
            }
        }
        if let Some(info) = heard {
            // An INFO0 after an INFO1 is a retrain's: a new phase 2.
            if matches!(info, Info::Info0(_) | Info::Info0d(_)) && settled.ended.is_some() {
                rounds.push(std::mem::take(&mut settled));
            }
            let event = info_event(&info, side, who, now, pcm_call, &mut settled);
            if settled.began.is_none() {
                settled.began = Some(event.start);
            }
            let points = points_between(&dpsk, (event.start * fs) as usize, n);
            let peak = points.iter().map(|(re, im)| re.hypot(*im)).fold(1e-9f32, f32::max);
            let scaled: Vec<(f32, f32)> = points.iter().map(|(re, im)| (re / peak, im / peak)).collect();
            let event = event.qam(&scaled, 2);
            // Each is sent until it is acknowledged: the repeats are one.
            match &last_info {
                Some((tag, at)) if *tag == event.tag && now - at < 1.0 => {
                    if let Some(prior) = events.iter_mut().rev().find(|e| e.tag == event.tag) {
                        prior.end = now;
                    }
                }
                _ => events.push(event.clone()),
            }
            last_info = Some((event.tag, now));
        }

        tone.feed(x);
        if n.is_multiple_of(every) {
            let (re, im) = tone.phasor();
            phasors.push((n, (re as f32, im as f32)));
        }
        probing.feed(x);
        let teeth = probing.standing(rms);
        if teeth {
            // The comb has tones 150 Hz either side of the carrier, and
            // they read as the carrier wandering.
            reversals.restart();
        } else if reversals.feed(x) {
            turns.push(n.saturating_sub(reversals.latency() as usize));
        }
        let amplitude = tone.amplitude();
        let on = rms > 1e-3 && amplitude > 1.2 * rms;
        match (&mut standing, on) {
            (Some(span), true) => {
                span.1 = n;
                span.2 = span.2.max(amplitude);
            }
            (None, true) => standing = Some((n, n, amplitude)),
            (Some(span), false) if n - span.1 > blink => {
                spans.push(*span);
                standing = None;
            }
            _ => {}
        }
        match (&mut comb, teeth) {
            (Some(span), true) => {
                span.1 = n;
                span.2 = span.2.max(rms);
            }
            (None, true) => comb = Some((n, n, rms)),
            (Some(span), false) if n - span.1 > blink => {
                combs.push(*span);
                comb = None;
            }
            _ => {}
        }
    }
    spans.extend(standing);
    combs.extend(comb);
    combs.retain(|c| (c.1 - c.0) as f64 / fs >= 0.2);

    // An INFO sequence is the same carrier, and where its bits run the same
    // for a while it is a tone, and where they change it is a reversal.
    // Neither is tone A or B: those are what is left once the sequences are
    // taken out.
    let pad = (0.02 * fs) as usize;
    let taken: Vec<(usize, usize)> = events
        .iter()
        .map(|e| (((e.start * fs) as usize).saturating_sub(pad), (e.end * fs) as usize + pad))
        .chain(combs.iter().map(|c| (c.0.saturating_sub(pad), c.1 + pad)))
        .collect();
    let mut clear: Vec<(usize, usize, f64)> = Vec::new();
    for (mut a, b, peak) in spans {
        let mut cuts: Vec<(usize, usize)> = taken.iter().copied().filter(|t| t.0 < b && t.1 > a).collect();
        cuts.sort_unstable();
        for (ta, tb) in cuts {
            if ta > a {
                clear.push((a, ta, peak));
            }
            a = a.max(tb);
        }
        if b > a {
            clear.push((a, b, peak));
        }
    }
    turns.retain(|&t| !taken.iter().any(|&(a, b)| t >= a && t <= b));
    // A reversal is a notch in the tone as well as a turn of it: the tone
    // either side of one is the same tone.
    let near = (0.06 * fs) as usize;
    let mut joined: Vec<(usize, usize, f64)> = Vec::new();
    for span in clear {
        match joined.last_mut() {
            Some(last)
                if span.0.saturating_sub(last.1) < 2 * near
                    && turns.iter().any(|&t| t + pad >= last.1 && t <= span.0 + pad) =>
            {
                last.1 = span.1;
                last.2 = last.2.max(span.2);
            }
            _ => joined.push(span),
        }
    }
    for (a, b, peak) in joined {
        let (start, end) = (a as f64 / fs, b as f64 / fs);
        if end - start < 0.04 {
            continue;
        }
        let mine: Vec<usize> = turns.iter().copied().filter(|&t| t + near >= a && t <= b + near).collect();
        let mut event = Event::new(start, end, who, Layer::Phase2, tone_tag)
            .summary(format!(
                "{tone_name}, {carrier:.0} Hz for {:.0} ms{}",
                1000.0 * (end - start),
                match mine.len() {
                    0 => String::new(),
                    1 => ", one phase reversal".to_owned(),
                    n => format!(", {n} phase reversals"),
                }
            ))
            .field("level", db(peak))
            .level(peak)
            .phasor(&points_between(&phasors, a, b))
            .band(carrier - 60.0, carrier + 60.0);
        for at in &mine {
            event = event.field("phase reversal at", format!("{:.3} s", *at as f64 / fs));
        }
        events.push(event);
        for at in mine {
            let t = at as f64 / fs;
            events.push(
                Event::new(t, t + 0.01, who, Layer::Phase2, format!("{tone_tag} rev"))
                    .summary(format!("{tone_name} reverses phase: a mark for the round-trip delay"))
                    .phasor(&points_between(&phasors, at.saturating_sub(near), at + near))
                    .level(peak)
                    .band(carrier - 200.0, carrier + 200.0),
            );
        }
    }
    for (a, b, level) in combs {
        probe_events(samples, fs, a, b, level, who, &mut events);
    }
    rounds.push(settled);
    (events, rounds)
}

/// L1 and L2 between samples `a` and `b`, with what the probing shows of the
/// path from the modem that sent it to the tap.
fn probe_events(samples: &[f32], fs: f64, a: usize, b: usize, level: f64, who: End, events: &mut Vec<Event>) {
    let (start, end) = (a as f64 / fs, b as f64 / fs);
    if end - start < 0.2 {
        return;
    }
    let l1_end = start + 0.160;
    events.push(
        Event::new(start, l1_end, who, Layer::Phase2, "L1")
            .summary("line probing L1: the comb of tones, 6 dB up, for 160 ms".to_owned())
            .level(level)
            .band(150.0, 3750.0),
    );
    let mut event = Event::new(l1_end, end, who, Layer::Phase2, "L2")
        .summary(format!("line probing L2 for {:.0} ms", 1000.0 * (end - l1_end)))
        .level(level)
        .band(150.0, 3750.0);
    let mut analyzer = Analyzer::new(fs);
    let from = ((l1_end + 0.02) * fs) as usize;
    let to = (b.saturating_sub((0.01 * fs) as usize)).min(samples.len());
    for &sample in samples.get(from..to).unwrap_or(&[]) {
        analyzer.feed(sample as f64);
    }
    if let Some(Reading { tones, frequency_offset }) = analyzer.reading() {
        let snrs: Vec<f64> = tones.iter().map(|t| t.snr_db).collect();
        let mut sorted = snrs.clone();
        sorted.sort_by(f64::total_cmp);
        if let Some(median) = sorted.get(sorted.len() / 2) {
            event = event.summary(format!(
                "line probing L2 for {:.0} ms: {median:.0} dB median signal to noise at the tap",
                1000.0 * (end - l1_end)
            ));
        }
        if let Some(offset) = frequency_offset {
            event = event.field("frequency offset", format!("{offset:+.2} Hz"));
        }
        for tone in &tones {
            event = event.field(
                format!("{:.0} Hz", tone.frequency),
                format!("{:+.1} dB, signal to noise {:.0} dB", tone.gain_db, tone.snr_db),
            );
        }
    }
    events.push(event);
}
