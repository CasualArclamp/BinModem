//! Phases 3 and 4 of a V.92 call whose upstream is PCM as well, overheard on
//! a recording with an end a channel.
//!
//! Both directions are PCM here, and both are read the way the modem at the
//! other end reads them, with BinModem's own V.92 receivers: the digital
//! modem's downstream as the analogue modem has it (Sd, TRN1d, Jd, Jp, Jp',
//! the DIL, Ri, then TRN2d, SUVd, CPd, Ed, B1d and data, V.90's encoder on
//! six constellations), and the analogue modem's upstream as the digital
//! modem has it (Ru, TRN1u, Ja, Su, the second TRN1u, CPt, E1u, then TRN2u,
//! SUVu, CPu, E2u, B1u and data, the 6.4 chain on CPd's constellations).
//!
//! Each needs something the other said. The DIL is as long as Ja asked,
//! TRN2d comes on the CPt's constellations and B1d on the CPu's; going up,
//! the second S-bar-u is stepped by Jp's epsilon and data mode is sent
//! through the CPd. Each was said before it was needed, so the two channels
//! are read in step, a symbol of each at a time, and what one has read is
//! there for the other. The one exception is the upstream's data-mode fit,
//! which is made on TRN2u before the CPd that shapes it has come down: so
//! the call is read twice, and the second time the upstream has the CPd the
//! first one found.
//!
//! Both are read at the network's 8000 symbols a second. A downstream
//! recorded faster is read on whichever sample of each symbol Sd says is the
//! codec's; an upstream recorded faster is brought down to 8000 a second,
//! where the instants are not known, so both half-symbol phases are tried and
//! the better kept.

use std::collections::VecDeque;

use datapump::v90::encoder::Mapping;
use datapump::v90::sequences::{Cp, CpFinder, Descriptor, DescriptorFinder, JA_UNIT_BITS, JD_BITS, Jd, Layout};
use datapump::v90::ucode::{self, Law};
use datapump::v92::analogue::{
    DOWN_SIGN, DownEvent, Downstream as DownReader, TRAIN_AFTER, TRAIN_SEARCH, TRN1D_START,
};
use datapump::v92::decoder::Decoder as UpDecoder;
use datapump::v92::precoder::Precoder;
use datapump::v92::receiver::{ALIGN_WINDOW, Heard, Receiver, Slicer, TRAIN_WINDOW};
use datapump::v92::sequences::{
    CpFamily, Cpd, DownFamily, J, JP_PRIME_BITS, Jp, SHORT_BITS, UpFinder, trn2u_unit,
};
use datapump::v92::up_signals::{Trn2uReader, Trn2uSize, TwoPointReader};
use datapump::v92::upstream::UpEncoder;
use datapump::v92::{Parameters, UP_INTERVALS, epsilon};
use dsp::{Complex, Resampler};

use crate::model::{End, Event, Layer};
use crate::pcm::{Outcome, cp_fields, descriptor_fields, jd_fields};
use crate::phase2::Settled;
use crate::v34::Stream;

/// The network's rate, which is both directions' symbol rate.
const BAUD: f64 = 8000.0;

/// TRN1u begins 24T after the Ru-to-R-bar-u reversal (9.5.1.1.2), and the
/// second one 24T after the S-bar-u that answers Jp.
const TRN_START: u64 = 24;

/// MD is counted in 276-symbol steps (Table 18).
const MD_STEP: u64 = 276;

/// The second TRN1u's refit: symbols of it before it is tried, how far the
/// alignment is looked for, the correlation it needs, and how much worse
/// than the first fit it may come out.
const REFIT_WINDOW: u64 = 600;
const REFIT_SEARCH: u64 = 192;
const REFIT_QUALITY: f64 = 0.5;
const REFIT_AHEAD: u64 = 12;
const REFIT_SLACK_DB: f64 = 3.0;

/// The zeros in a row that are E1u (8.5.8).
const E1U_ZEROES: usize = 12;

/// Phase 4's upstream fit is made this far into TRN2u, and the sequences are
/// read only from a little after it.
const TRN2U_FIT_AFTER: u64 = 2400;
const TRN2U_DROP_AFTER_FIT: u64 = 300;

/// B1u's data frames the decoder is not given while the equaliser settles
/// onto the data constellation: B1u is ones and says nothing.
const B1U_ACQUIRE_FRAMES: usize = 18;

/// How long after Ed went down B1u is started without an E2u having been
/// read, in symbols.
const B1U_WAIT: u64 = 16_000;

/// How many symbols either side of the one after E2u B1u is looked for.
const SPREAD: usize = 3;

/// The line's levels kept from before B1u is placed: the receiver's own
/// delay is somewhere inside it.
const RECENT: usize = 256;

/// How far past where E2u was read B1u is looked for.
const AHEAD: usize = 2000;

/// The equaliser fitted on B1u.
const EQ_TAPS: usize = 24;

/// Bits of what is left of B1u each delay is judged on.
const JUDGED: usize = 1500;

/// Levels kept for a symbol plot.
const KEPT: usize = 60_000;

/// What each direction has read that the other is waiting for.
#[derive(Debug, Default)]
struct Shared {
    descriptor: Option<Descriptor>,
    upstream_rates: Option<u32>,
    jp: Option<Jp>,
    cpt: Option<Cp>,
    cpu: Option<Cp>,
    cpd: Option<Cpd>,
    /// Whether the digital modem has sent an acknowledged SUVd or CPd, which
    /// is what E2u waits for.
    down_acked: bool,
    /// The upstream symbol Ed went down at.
    ed_at: Option<u64>,
}

/// Something either direction says about itself, repeated sequences being one
/// event.
#[derive(Debug, Default)]
struct Log {
    events: Vec<Event>,
}

impl Log {
    fn push(&mut self, event: Event) {
        self.events.push(event);
    }

    /// A sequence sent again and again: one event that grows, while each
    /// repetition comes within half a second of the last.
    fn repeat(&mut self, event: Event) {
        if let Some(last) = self.events.iter_mut().rev().find(|e| e.tag == event.tag && e.from == event.from)
            && event.start - last.end < 0.5
        {
            last.end = event.end;
            let count = last
                .fields
                .iter()
                .position(|(name, _)| name == "repetitions read")
                .map(|i| last.fields.remove(i).1.parse::<usize>().unwrap_or(1))
                .unwrap_or(1);
            last.fields.push(("repetitions read".to_owned(), (count + 1).to_string()));
            return;
        }
        self.events.push(event);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DownStage {
    /// Waiting for Sd to turn over.
    Sd,
    /// TRN1d is on the line: fitting to it.
    Trn1d,
    /// Jd and then Jp, in the two-point modulation TRN1d set up.
    J,
    /// Jp has been read: Jp' ends it.
    AfterJp,
    Dil,
    /// The DIL is over: Ri and its reversal.
    Ri,
    Phase4,
}

/// The digital modem's signal, as the analogue modem hears it.
struct Down {
    who: End,
    law: Law,
    unit: f64,
    rx: Receiver,
    stage: DownStage,
    log: Log,
    sd_from: Option<f64>,
    s_bar_d: f64,
    trn_from: u64,
    reader: TwoPointReader,
    window: VecDeque<bool>,
    jd: bool,
    jp_at: f64,
    zeroes: usize,
    dil_left: usize,
    dil_from: f64,
    dil: Vec<f32>,
    ri_from: Option<f64>,
    frames: Option<DownReader>,
    cpu_given: bool,
    trn2d_from: f64,
    b1d_from: f64,
    data_from: Option<f64>,
    data_levels: Vec<f32>,
    stream: Stream,
    bps: Option<u32>,
}

impl Down {
    fn new(who: End, law: Law, uinfo: u8) -> Self {
        let mut rx = Receiver::new();
        rx.set_reference_mode(datapump::v32::Mode::Call);
        rx.expect_su();
        Self {
            who,
            law,
            unit: ucode::level(law, uinfo),
            rx,
            stage: DownStage::Sd,
            log: Log::default(),
            sd_from: None,
            s_bar_d: 0.0,
            trn_from: 0,
            reader: TwoPointReader::differential_with(datapump::v32::Mode::Call),
            window: VecDeque::new(),
            jd: false,
            jp_at: 0.0,
            zeroes: 0,
            dil_left: 0,
            dil_from: 0.0,
            dil: Vec::new(),
            ri_from: None,
            frames: None,
            cpu_given: false,
            trn2d_from: 0.0,
            b1d_from: 0.0,
            data_from: None,
            data_levels: Vec::new(),
            stream: Stream { from: Some(who), ..Stream::default() },
            bps: None,
        }
    }

    fn event(&self, start: f64, end: f64, layer: Layer, tag: &str) -> Event {
        Event::new(start, end.max(start + 0.001), self.who, layer, tag)
    }

    fn feed(&mut self, now: f64, level: f64, shared: &mut Shared) {
        self.rx.feed(level);
        while let Some(heard) = self.rx.heard() {
            self.heard(now, heard, shared);
        }
        if self.stage == DownStage::Trn1d && self.rx.symbols() >= self.trn_from + TRAIN_AFTER {
            let base = self.rx.symbols() - self.rx.history_len() as u64;
            if self.rx.train_near(self.trn_from.saturating_sub(base), TRAIN_SEARCH) {
                let snr = self.rx.snr_db();
                self.log.push(
                    self.event(self.s_bar_d, now, Layer::Phase3, "TRN1d")
                        .summary(format!("TRN1d: the analogue modem's receiver trains on it, {snr:.1} dB"))
                        .field("fit", format!("{snr:.1} dB")),
                );
                self.reader = TwoPointReader::differential_with(datapump::v32::Mode::Call);
                self.window.clear();
                self.stage = DownStage::J;
            } else {
                self.trn_from += TRAIN_AFTER / 2;
            }
        }
        if self.stage == DownStage::Phase4
            && !self.cpu_given
            && let (Some(frames), Some(cpu)) = (self.frames.as_mut(), shared.cpu.as_ref())
        {
            self.cpu_given = true;
            if frames.set_cpu(cpu, self.law) {
                self.bps = Mapping::from_cp(cpu).map(|m| m.rate());
            }
        }
    }

    fn heard(&mut self, now: f64, heard: Heard, shared: &mut Shared) {
        match heard {
            Heard::Su { .. } if self.stage == DownStage::Sd => {
                self.sd_from.get_or_insert(now);
            }
            Heard::SuReversal { at } if self.stage == DownStage::Sd => {
                let from = self.sd_from.unwrap_or(now - 0.05);
                self.log.push(
                    self.event(from, now, Layer::Phase3, "Sd")
                        .summary(format!("Sd for {:.0} ms: the digital modem's S", 1000.0 * (now - from))),
                );
                self.s_bar_d = now;
                self.trn_from = at + TRN1D_START;
                self.stage = DownStage::Trn1d;
            }
            Heard::Ru { .. } if self.stage == DownStage::Ri => {
                self.ri_from.get_or_insert(now);
            }
            Heard::RuReversal { .. } if self.stage == DownStage::Ri => {
                let from = self.ri_from.unwrap_or(now - 0.048);
                self.log.push(
                    self.event(from, now, Layer::Phase3, "Ri")
                        .summary("Ri: the digital modem has its CPt, and phase 4 follows".to_owned()),
                );
                self.begin_phase4(now, shared);
            }
            Heard::Symbol(symbol) => self.symbol(now, symbol.value, shared),
            _ => {}
        }
    }

    fn begin_phase4(&mut self, now: f64, shared: &Shared) {
        self.stage = DownStage::Phase4;
        self.rx.hand_over_unsliced();
        self.trn2d_from = now;
        match shared.cpt.as_ref().and_then(|cpt| DownReader::overheard(cpt, self.law)) {
            Some(frames) => self.frames = Some(frames),
            None => self.log.push(self.event(now, now, Layer::Phase4, "TRN2d").summary(
                "phase 4 follows, but the CPt it is sent on was not read from the analogue modem: not followed".to_owned(),
            )),
        }
    }

    fn symbol(&mut self, now: f64, value: f64, shared: &mut Shared) {
        match self.stage {
            DownStage::J | DownStage::AfterJp => {
                let Some(bit) = self.reader.feed(value) else { return };
                self.window.push_back(bit);
                while self.window.len() > JD_BITS {
                    self.window.pop_front();
                }
                let j = (self.window.len() == JD_BITS).then(|| {
                    let bits: Vec<bool> = self.window.iter().copied().collect();
                    J::from_bits(&bits)
                });
                if self.stage == DownStage::J {
                    match j.flatten() {
                        Some(J::Jd(jd)) if !self.jd => {
                            self.jd = true;
                            self.log.push(self.jd_event(now, &jd));
                        }
                        Some(J::Jp(jp)) => {
                            self.jp_at = now;
                            shared.jp = Some(jp);
                            self.log.push(
                                self.event(now - 72.0 / BAUD, now, Layer::Phase3, "Jp")
                                    .summary(format!(
                                        "Jp: the analogue modem's S-bar-u is to be {:.3} of a symbol longer",
                                        f64::from(jp.epsilon) / 65536.0
                                    ))
                                    .field("epsilon", format!("{} / 65536 of a symbol", jp.epsilon))
                                    .field(
                                        "TRN2u, SUVu and CPu on",
                                        if jp.eight_in_training { "8 points" } else { "4 points" },
                                    ),
                            );
                            self.zeroes = 0;
                            self.stage = DownStage::AfterJp;
                            // Ri is hunted from here; it is only taken once
                            // the DIL is over, which a DIL segment can look
                            // like.
                            self.rx.expect_ru();
                        }
                        _ => {}
                    }
                    return;
                }
                // Jp' is twelve zeros after a whole Jp.
                if matches!(j.flatten(), Some(J::Jp(_))) {
                    self.zeroes = 0;
                } else if bit {
                    self.zeroes = usize::MAX;
                } else if self.zeroes != usize::MAX {
                    self.zeroes += 1;
                }
                if self.zeroes == JP_PRIME_BITS {
                    self.log.push(
                        self.event(now - JP_PRIME_BITS as f64 / BAUD, now, Layer::Phase3, "Jp'")
                            .summary("Jp': the end of Jp, and the DIL follows".to_owned()),
                    );
                    self.dil_left = shared.descriptor.as_ref().map_or(0, Descriptor::len);
                    self.dil_from = now;
                    self.stage = if self.dil_left > 0 { DownStage::Dil } else { DownStage::Ri };
                    if shared.descriptor.is_none() {
                        self.log.push(self.event(now, now, Layer::Phase3, "DIL").summary(
                            "the analogue modem's Ja was not read, so how long the DIL is is not known".to_owned(),
                        ));
                    }
                }
            }
            DownStage::Dil => {
                self.dil.push((value * self.unit) as f32);
                self.dil_left -= 1;
                if self.dil_left == 0 {
                    let levels = std::mem::take(&mut self.dil);
                    self.log.push(
                        self.event(self.dil_from, now, Layer::Phase3, "DIL")
                            .pcm(&levels)
                            .summary(format!("DIL for {:.2} s: {} symbols", now - self.dil_from, levels.len()))
                            .field("symbols", levels.len().to_string()),
                    );
                    self.stage = DownStage::Ri;
                }
            }
            DownStage::Phase4 => {
                let level = DOWN_SIGN * value * self.unit;
                let Some(frames) = self.frames.as_mut() else { return };
                if self.data_from.is_some() && self.data_levels.len() < KEPT {
                    self.data_levels.push(level as f32);
                }
                if let Some(event) = frames.feed(level) {
                    self.down_event(now, event, shared);
                }
            }
            _ => {}
        }
    }

    fn jd_event(&self, now: f64, jd: &Jd) -> Event {
        self.event(now - 72.0 / BAUD, now, Layer::Phase3, "Jd")
            .summary("Jd: the downstream rates the digital modem can send".to_owned())
            .fields(jd_fields(jd))
    }

    fn down_event(&mut self, now: f64, event: DownEvent, shared: &mut Shared) {
        match event {
            DownEvent::Found => {
                self.log.push(
                    self.event(self.trn2d_from, now, Layer::Phase4, "TRN2d")
                        .summary("TRN2d: the downstream on the CPt's constellations".to_owned()),
                );
            }
            DownEvent::Sequence(family) => match *family {
                DownFamily::Suvd(suvd) => {
                    shared.down_acked |= suvd.ack;
                    self.log.repeat(
                        self.event(now - 0.01, now, Layer::Phase4, "SUVd")
                            .summary(format!(
                                "SUVd{}: the digital modem's state of the exchange",
                                if suvd.ack { "'" } else { "" }
                            ))
                            .field("acknowledges", if suvd.ack { "yes" } else { "no" }),
                    );
                }
                DownFamily::Cpd(cpd) => {
                    shared.down_acked |= cpd.ack;
                    let merged = match shared.cpd.as_ref() {
                        Some(before) => cpd.merged_over(before),
                        None => cpd,
                    };
                    let mut event = self
                        .event(now - 0.01, now, Layer::Phase4, "CPd")
                        .fields(cpd_fields(&merged));
                    event = match Parameters::try_from(&merged) {
                        Ok(p) => event.summary(format!("CPd: the analogue modem is to send {} bit/s", p.rate())),
                        Err(why) => event.summary(format!("CPd, not all of it yet: {why}")),
                    };
                    shared.cpd = Some(merged);
                    self.log.repeat(event);
                }
            },
            DownEvent::Ed => {
                shared.ed_at = Some(0);
                self.log.push(
                    self.event(now - 12.0 / BAUD, now, Layer::Phase4, "Ed")
                        .summary("Ed: the digital modem's exchange is over".to_owned()),
                );
            }
            DownEvent::B1dBegun { .. } => self.b1d_from = now,
            DownEvent::B1d { clean } => {
                self.log.push(
                    self.event(self.b1d_from, now, Layer::Phase4, "B1d")
                        .summary(format!("B1d: {clean} of 48 frames read as ones"))
                        .field("frames of ones", format!("{clean} of 48")),
                );
                self.data_from = Some(now);
                self.stream.bps = self.bps;
                let mut data = self.event(now, now, Layer::Data, "data");
                data = match self.bps {
                    Some(bps) => data
                        .summary(format!("data mode down at {bps} bit/s"))
                        .field("rate", format!("{bps} bit/s")),
                    None => data.summary("data mode down".to_owned()),
                };
                self.log.push(data);
            }
            DownEvent::Data(bits) => {
                if self.stream.bits.len().is_multiple_of(256) || self.stream.marks.is_empty() {
                    self.stream.marks.push((self.stream.bits.len(), now));
                }
                self.stream.bits.extend(bits);
                if let Some(data) = self.log.events.iter_mut().rev().find(|e| e.tag == "data") {
                    data.end = now;
                }
            }
        }
    }

    fn finish(&mut self, now: f64) {
        self.stream.marks.push((self.stream.bits.len(), now));
        let levels = std::mem::take(&mut self.data_levels);
        if let Some(data) = self.log.events.iter_mut().rev().find(|e| e.tag == "data") {
            *data = data.clone().pcm(&levels);
        }
    }
}

fn cpd_fields(cpd: &Cpd) -> Vec<(String, String)> {
    let mut fields = vec![
        ("rate number".to_owned(), cpd.drn.to_string()),
        ("trellis".to_owned(), format!("{:?}", cpd.trellis)),
        ("acknowledges".to_owned(), if cpd.ack { "yes" } else { "no" }.to_owned()),
        ("E2u one symbol longer".to_owned(), if cpd.extend_e2u { "yes" } else { "no" }.to_owned()),
        ("gain, 4G".to_owned(), cpd.gain4.to_string()),
    ];
    if let Some(filters) = &cpd.filters {
        fields.push((
            "filter taps".to_owned(),
            format!("z1 {}, p1 {}, z2 {}, p2 {}", filters.z1.len(), filters.p1.len(), filters.z2.len(), filters.p2.len()),
        ));
    }
    if let Some(sets) = &cpd.sets {
        fields.push(("constellations".to_owned(), format!("{:?} points, index {:?}", sets.points.iter().map(Vec::len).collect::<Vec<_>>(), sets.index)));
    }
    if let Some(moduli) = &cpd.moduli {
        fields.push(("moduli".to_owned(), format!("{moduli:?}")));
    }
    fields
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpStage {
    Ru,
    Md(u64),
    Trn1u,
    Ja,
    Su,
    SuBar,
    /// The second TRN1u, the DIL's time, CPt and E1u.
    Cpt,
    Phase4,
    Data,
}

/// The analogue modem's signal, as the digital modem receives it.
struct Up {
    who: End,
    md: u64,
    rx: Receiver,
    stage: UpStage,
    log: Log,
    ru_from: Option<f64>,
    trn1u_from: f64,
    trn_from: u64,
    trn1u_snr: Option<f64>,
    reader: TwoPointReader,
    ja: DescriptorFinder,
    su_from: Option<f64>,
    stepped: bool,
    refit_from: Option<u64>,
    refit_tries: u32,
    cps: CpFinder,
    cp_len: usize,
    cp_pos: usize,
    since_cpt: usize,
    e1u_zeroes: usize,
    cpt_read: bool,
    trn2u_from: u64,
    trn2u_at: f64,
    four_at: u64,
    drop_until: u64,
    size: Trn2uSize,
    trn2u: Option<Trn2uReader>,
    finder: Option<UpFinder>,
    e2u_target: Option<u32>,
    e2u_zeroes: u32,
    /// The line's levels lately read, before any equaliser.
    recent: VecDeque<f64>,
    /// From a little before B1u: the line's levels, and B1u as it was sent.
    raw: Vec<f64>,
    b1u: Vec<f64>,
    /// The equaliser fitted on B1u, and where in `raw` B1u's first symbol is.
    taps: Option<(Vec<f64>, usize)>,
    /// How far through `raw` the data has been read.
    done: usize,
    fit_db: f64,
    restorer: Option<Restorer>,
    /// B1u's first symbol is taken to be at each of a few places either side
    /// of the symbol after E2u, and each is read.
    candidates: Vec<Candidate>,
    chosen: Option<usize>,
    b1u_at: f64,
    /// Symbols since the one after E2u.
    since_e2u: usize,
    levels: Vec<f32>,
    stream: Stream,
    bps: Option<u32>,
    /// The CPd from an earlier reading of the call.
    known_cpd: Option<Cpd>,
}

impl Up {
    fn new(who: End, md: u8, known_cpd: Option<Cpd>) -> Self {
        let mut rx = Receiver::new();
        rx.expect_ru();
        rx.expect_step(0.0);
        Self {
            who,
            md: u64::from(md),
            rx,
            stage: UpStage::Ru,
            log: Log::default(),
            ru_from: None,
            trn1u_from: 0.0,
            trn_from: 0,
            trn1u_snr: None,
            reader: TwoPointReader::differential(),
            ja: DescriptorFinder::v92(),
            su_from: None,
            stepped: false,
            refit_from: None,
            refit_tries: 0,
            cps: CpFinder::v92(JA_UNIT_BITS),
            cp_len: 0,
            cp_pos: 0,
            since_cpt: 0,
            e1u_zeroes: 0,
            cpt_read: false,
            trn2u_from: 0,
            trn2u_at: 0.0,
            four_at: u64::MAX,
            drop_until: 0,
            size: Trn2uSize::Four,
            trn2u: None,
            finder: None,
            e2u_target: None,
            e2u_zeroes: 0,
            recent: VecDeque::new(),
            raw: Vec::new(),
            b1u: Vec::new(),
            taps: None,
            done: 0,
            fit_db: 0.0,
            restorer: None,
            candidates: Vec::new(),
            chosen: None,
            b1u_at: 0.0,
            since_e2u: 0,
            levels: Vec::new(),
            stream: Stream { from: Some(who), ..Stream::default() },
            bps: None,
            known_cpd,
        }
    }

    fn event(&self, start: f64, end: f64, layer: Layer, tag: &str) -> Event {
        Event::new(start, end.max(start + 0.001), self.who, layer, tag)
    }

    fn feed(&mut self, now: f64, level: f64, shared: &mut Shared) {
        if self.stage == UpStage::Data {
            self.raw_symbol(now, level);
            return;
        }
        self.recent.push_back(level);
        while self.recent.len() > RECENT {
            self.recent.pop_front();
        }
        self.rx.feed(level);
        while let Some(heard) = self.rx.heard() {
            self.heard(now, heard, shared);
        }
        let symbols = self.rx.symbols();
        match self.stage {
            UpStage::Md(until) if symbols >= until => {
                self.stage = UpStage::Ru;
                self.rx.restart_hunt();
            }
            UpStage::Trn1u if symbols >= self.trn_from + TRAIN_AFTER => {
                let base = symbols - self.rx.history_len() as u64;
                if self.rx.train_near(self.trn_from.saturating_sub(base), TRAIN_SEARCH) {
                    let snr = self.rx.snr_db();
                    self.trn1u_snr = Some(snr);
                    self.log.push(
                        self.event(self.trn1u_from, now, Layer::Phase3, "TRN1u")
                            .summary(format!("TRN1u: the digital modem's receiver trains on it, {snr:.1} dB"))
                            .field("fit", format!("{snr:.1} dB")),
                    );
                    self.stage = UpStage::Ja;
                } else {
                    self.trn_from += TRAIN_AFTER / 2;
                }
            }
            _ => {}
        }
        // Jp's epsilon is the step the S-bar-u answering it takes.
        if !self.stepped
            && matches!(self.stage, UpStage::Su | UpStage::SuBar)
            && let Some(jp) = shared.jp
        {
            self.stepped = true;
            self.rx.expect_step(epsilon::step_direction() * f64::from(jp.epsilon) / 65536.0);
        }
        if self.stage == UpStage::Cpt
            && let Some(from) = self.refit_from
            && symbols >= from + REFIT_WINDOW
        {
            self.refit(from);
        }
        if self.stage == UpStage::Phase4 && symbols >= self.four_at {
            self.four_at = u64::MAX;
            if self.rx.train_on_trn2u(TRAIN_WINDOW, self.trn2u_from) && self.rx.install_phase_four() {
                self.drop_until = symbols + TRN2U_DROP_AFTER_FIT;
                if self.rx.trn2u_sign_first() {
                    self.trn2u = Some(Trn2uReader::new(self.size).with_bit_order(false));
                }
            }
        }
        if self.stage == UpStage::Phase4
            && self.candidates.is_empty()
            && shared.cpd.is_some()
            && let Some(ed) = shared.ed_at
            && ed > 0
            && symbols > ed + B1U_WAIT
        {
            self.begin_b1u(now, shared);
        }
        if shared.ed_at == Some(0) {
            shared.ed_at = Some(symbols);
        }
    }

    fn heard(&mut self, now: f64, heard: Heard, shared: &mut Shared) {
        match heard {
            Heard::Ru { .. } if self.stage == UpStage::Ru => {
                self.ru_from.get_or_insert(now);
            }
            Heard::RuReversal { at } if self.stage == UpStage::Ru => {
                let from = self.ru_from.take().unwrap_or(now - 0.048);
                self.log.push(
                    self.event(from, now, Layer::Phase3, "Ru")
                        .summary("Ru: the analogue modem's opening, and its reversal".to_owned()),
                );
                if self.md > 0 {
                    let until = self.rx.symbols() + self.md * MD_STEP;
                    self.md = 0;
                    self.stage = UpStage::Md(until);
                    self.rx.restart_hunt();
                    return;
                }
                self.trn_from = at + TRN_START;
                self.trn1u_from = now;
                self.stage = UpStage::Trn1u;
                self.rx.expect_su();
                self.rx.expect_step(epsilon::step_direction() * 0.5);
                self.rx.restart_hunt();
            }
            Heard::Su { .. } if self.stage == UpStage::Su => {
                self.su_from.get_or_insert(now);
            }
            Heard::SuReversal { .. } if self.stage == UpStage::Su => {
                let from = self.su_from.unwrap_or(now - 0.018);
                self.log.push(
                    self.event(from, now, Layer::Phase3, "Su")
                        .summary("Su, and the 24.5-symbol S-bar-u the digital modem measures epsilon on".to_owned()),
                );
                self.stage = UpStage::SuBar;
            }
            Heard::SuReversal { at } if self.stage == UpStage::SuBar => {
                self.log.push(
                    self.event(now - 0.003, now, Layer::Phase3, "S-bar-u")
                        .summary("S-bar-u, stepped by Jp's epsilon: the second TRN1u follows".to_owned()),
                );
                self.refit_from = Some(at + TRN_START);
                self.refit_tries = 0;
                self.reader = TwoPointReader::differential();
                self.cps = CpFinder::v92(JA_UNIT_BITS);
                self.cp_len = 0;
                self.cp_pos = 0;
                self.e1u_zeroes = 0;
                self.rx.far_end_stepped();
                self.stage = UpStage::Cpt;
            }
            Heard::Symbol(symbol) => self.symbol(now, symbol.value, shared),
            _ => {}
        }
    }

    fn refit(&mut self, from: u64) {
        let base = self.rx.history_base();
        let at = from.saturating_sub(base);
        let before = self.rx.fitted();
        let baseline = before.snr();
        let found = match self.rx.align_trn1u(at, REFIT_SEARCH, ALIGN_WINDOW) {
            Some((aligned, quality)) if quality > REFIT_QUALITY => {
                self.rx.set_frame_origin(base + aligned);
                self.rx
                    .train_verified_ahead(aligned, TRAIN_WINDOW, REFIT_AHEAD)
                    .then(|| self.rx.snr_db())
            }
            _ => None,
        };
        match found {
            Some(snr) if snr > baseline - REFIT_SLACK_DB => self.refit_from = None,
            _ => {
                self.rx.restore(&before);
                self.refit_tries += 1;
                self.refit_from = (self.refit_tries < 3).then_some(from + REFIT_WINDOW / 2);
            }
        }
    }

    fn symbol(&mut self, now: f64, value: f64, shared: &mut Shared) {
        match self.stage {
            UpStage::Ja => {
                let Some(bit) = self.reader.feed(value) else { return };
                if let Some(descriptor) = self.ja.feed(bit) {
                    shared.upstream_rates = self.ja.upstream_rates();
                    let mut event = self
                        .event(now - 0.05, now, Layer::Phase3, "Ja")
                        .summary(format!(
                            "Ja: {} for the DIL",
                            if descriptor.is_empty() {
                                "no DIL asked".to_owned()
                            } else {
                                format!("{} symbols asked", descriptor.len())
                            }
                        ))
                        .fields(descriptor_fields(&descriptor));
                    if let Some(rates) = shared.upstream_rates {
                        event = event.field("upstream rates allowed", format!("{rates:05x}"));
                    }
                    self.log.push(event);
                    shared.descriptor = Some(descriptor);
                    self.rx.restart_hunt();
                    self.stage = UpStage::Su;
                }
            }
            UpStage::Cpt => {
                let Some(bit) = self.reader.feed(value) else { return };
                self.cpt_bit(now, bit, shared);
            }
            UpStage::Phase4 => self.phase4_symbol(now, value, shared),
            _ => {}
        }
    }

    fn cpt_bit(&mut self, now: f64, bit: bool, shared: &mut Shared) {
        if let Some(cp) = self.cps.feed(bit) {
            self.cp_len = cp.to_bits_in(Layout::V92, UP_INTERVALS).len();
            self.cp_pos = 0;
            self.since_cpt = 0;
            self.e1u_zeroes = 0;
            self.cpt_read = true;
            self.log.repeat(
                self.event(now - self.cp_len as f64 / BAUD, now, Layer::Phase3, "CPt")
                    .summary(format!(
                        "CPt: TRN2d and the exchange to come down at rate {}",
                        cp.drn
                    ))
                    .fields(cp_fields(&cp)),
            );
            shared.cpt = Some(cp);
            return;
        }
        self.since_cpt = self.since_cpt.saturating_add(1);
        if self.cp_len == 0 {
            return;
        }
        if self.cp_pos < E1U_ZEROES {
            self.e1u_zeroes = if bit { 0 } else { self.e1u_zeroes + 1 };
            let fresh = self.since_cpt <= self.cp_len + E1U_ZEROES;
            if self.e1u_zeroes == E1U_ZEROES && fresh && self.cpt_read {
                self.log.push(
                    self.event(now - E1U_ZEROES as f64 / BAUD, now, Layer::Phase3, "E1u")
                        .summary("E1u: the end of phase 3 going up".to_owned()),
                );
                self.begin_upstream(now, shared);
                return;
            }
        }
        self.cp_pos += 1;
        if self.cp_pos >= self.cp_len {
            self.cp_pos = 0;
            self.e1u_zeroes = 0;
        }
    }

    fn begin_upstream(&mut self, now: f64, shared: &Shared) {
        if let Some(negative) = self.reader.last_sign() {
            self.rx.set_trn2u_seed(negative);
        }
        self.rx.begin_phase_four();
        let start = self.rx.symbols();
        self.trn2u_from = start;
        self.trn2u_at = now;
        self.four_at = start + TRN2U_FIT_AFTER;
        self.drop_until = start + TRN2U_FIT_AFTER + TRN2U_DROP_AFTER_FIT;
        self.size = if shared.jp.is_some_and(|jp| jp.eight_in_training) { Trn2uSize::Eight } else { Trn2uSize::Four };
        self.trn2u = Some(Trn2uReader::new(self.size));
        self.finder = Some(UpFinder::new(trn2u_unit(self.size == Trn2uSize::Eight)));
        self.rx.set_slicer(if self.size == Trn2uSize::Eight { Slicer::EightPoint } else { Slicer::FourPoint });
        self.stage = UpStage::Phase4;
    }

    /// The CPd the analogue modem sends with, as the wire carried it.
    fn parameters(&self, shared: &Shared) -> Option<Parameters> {
        let cpd = shared.cpd.as_ref().or(self.known_cpd.as_ref())?;
        Parameters::try_from(cpd).ok()
    }

    fn phase4_symbol(&mut self, now: f64, value: f64, shared: &mut Shared) {
        let _ = value;
        let Some(reader) = self.trn2u.as_mut() else { return };
        let bits: Vec<bool> = reader.feed(value).into_iter().collect();
        if self.rx.symbols() < self.drop_until {
            return;
        }
        if self.log.events.iter().all(|e| e.tag != "TRN2u") {
            let snr = self.rx.snr_db();
            self.log.push(
                self.event(self.trn2u_at, now, Layer::Phase4, "TRN2u")
                    .summary(format!("TRN2u: the digital modem's phase 4 fit, {snr:.1} dB"))
                    .field("fit", format!("{snr:.1} dB"))
                    .field("points", if self.size == Trn2uSize::Eight { "8" } else { "4" }),
            );
        }
        let mut started = false;
        for bit in bits {
            if let Some(finder) = self.finder.as_mut()
                && let Some(family) = finder.feed(bit)
            {
                let width = self.size.bits() as u32;
                let extended = self.parameters(shared).is_some_and(|p| p.extend_e2u);
                let e2u = width * (UP_INTERVALS as u32 + u32::from(extended));
                let unit = trn2u_unit(self.size == Trn2uSize::Eight);
                let fill = match family {
                    CpFamily::Suvu(_) | CpFamily::Cpus(_) => (SHORT_BITS.next_multiple_of(unit.max(1)) - SHORT_BITS) as u32,
                    CpFamily::Cpu(_) | CpFamily::Cpt(_) => 0,
                };
                self.e2u_target = Some(fill + e2u);
                self.e2u_zeroes = 0;
                self.up_sequence(now, family, shared);
                continue;
            }
            let Some(target) = self.e2u_target else { continue };
            if bit {
                self.e2u_target = None;
                continue;
            }
            self.e2u_zeroes += 1;
            if self.e2u_zeroes == target {
                self.e2u_target = None;
                if shared.down_acked && self.candidates.is_empty() {
                    self.log.push(
                        self.event(now - 12.0 / BAUD, now, Layer::Phase4, "E2u")
                            .summary("E2u: the analogue modem's exchange is over".to_owned()),
                    );
                    started = true;
                }
            }
        }
        if started {
            self.begin_b1u(now, shared);
        }
    }

    fn up_sequence(&mut self, now: f64, family: CpFamily, shared: &mut Shared) {
        match family {
            CpFamily::Suvu(suvu) => self.log.repeat(
                self.event(now - 0.01, now, Layer::Phase4, "SUVu")
                    .summary("SUVu: the analogue modem's state of the exchange".to_owned())
                    .field("waiting for a CPu", if suvu.wait_for_cpu { "yes" } else { "no" }),
            ),
            CpFamily::Cpu(cp) => {
                let rate = Mapping::from_cp(&cp).map(|m| m.rate());
                self.log.repeat(
                    self.event(now - 0.01, now, Layer::Phase4, "CPu")
                        .summary(match rate {
                            Some(bps) => format!("CPu: data mode to come down at {bps} bit/s"),
                            None => "CPu: the downstream's data mode".to_owned(),
                        })
                        .fields(cp_fields(&cp)),
                );
                shared.cpu = Some(cp);
            }
            CpFamily::Cpus(cpus) => self.log.repeat(
                self.event(now - 0.01, now, Layer::Phase4, "CPus")
                    .summary(format!("CPus: a short CPu, rate {}", cpus.drn)),
            ),
            CpFamily::Cpt(cp) => {
                shared.cpt = Some(cp);
            }
        }
    }

    /// B1u from about here (8.7.1).
    ///
    /// The digital modem reads data mode through its equaliser and the 6.4
    /// chain it designed, with a design delay of its own. B1u is known to the
    /// symbol, though: forty-eight frames of ones through the chain CPd
    /// describes, every memory starting from zero. So a short equaliser is
    /// fitted to it straight from the line, which puts its output at G v(n),
    /// the prefilter's output; and the levels the decoder wants are had from
    /// that with no decision at all. The prefilter's feed-forward section is
    /// inverted by least squares, its feedback put back, and that gives x(n);
    /// the precoder's tail, which is x(n) less the level u(n) it was built
    /// round, is a sum over earlier x and u, and is taken off.
    fn begin_b1u(&mut self, now: f64, shared: &Shared) {
        let Some(parameters) = self.parameters(shared) else { return };
        let Some(restorer) = Restorer::new(&parameters) else { return };
        let Ok(mut encoder) = UpEncoder::new(&parameters) else { return };
        self.b1u = encoder.b1u().iter().map(|symbol| symbol.out).collect();
        if let Some(path) = std::env::var_os("REVIEW_V92_B1U") {
            let text: String = self.b1u.iter().map(|v| format!("{v}
")).collect();
            let _ = std::fs::write(path, text);
        }
        // What the receiver has been given and not yet put out is in here
        // too: B1u's first symbol is some way back.
        self.raw = self.recent.drain(..).collect();
        self.taps = None;
        self.done = 0;
        self.candidates = (-1..=1)
            .map(|offset| Candidate {
                offset,
                precoder: Precoder::new(&parameters.filters),
                decoder: UpDecoder::new(&parameters),
                n: 0,
                ones: 0,
                held: Vec::new(),
            })
            .collect();
        self.restorer = Some(restorer);
        self.chosen = None;
        self.since_e2u = 0;
        self.b1u_at = now;
        self.bps = Some(parameters.rate());
        self.stage = UpStage::Data;
        self.log.push(
            self.event(now, now, Layer::Phase4, "B1u")
                .summary(format!("B1u, then data going up at {} bit/s", parameters.rate())),
        );
    }

    fn raw_symbol(&mut self, now: f64, level: f64) {
        self.raw.push(level);
        if self.taps.is_none() {
            if self.raw.len() < RECENT + AHEAD + self.b1u.len() + EQ_TAPS {
                return;
            }
            let Some((taps, start, db)) = fit_b1u(&self.raw, &self.b1u) else {
                self.stage = UpStage::Phase4;
                self.candidates.clear();
                return;
            };
            self.fit_db = db;
            self.done = start;
            self.taps = Some((taps, start));
        }
        // Every level the equaliser has its whole span of.
        while self.done + EQ_TAPS / 2 < self.raw.len() {
            let Some((taps, _)) = self.taps.as_ref() else { return };
            let centre = self.done;
            let value: f64 = taps
                .iter()
                .enumerate()
                .map(|(j, t)| t * self.raw.get((centre + j).wrapping_sub(EQ_TAPS / 2)).copied().unwrap_or(0.0))
                .sum();
            self.done += 1;
            self.data_symbol(now, value);
        }
    }

    fn data_symbol(&mut self, now: f64, value: f64) {
        if self.levels.len() < KEPT {
            self.levels.push(value as f32);
        }
        let Some(restorer) = self.restorer.as_mut() else { return };
        let x = restorer.step(value);
        let gain = restorer.gain;
        // `x` is x(m) for the symbol `delay` back, m counted from B1u's first.
        let m = self.since_e2u as i64 - restorer.delay as i64;
        self.since_e2u += 1;
        let acquire = (B1U_ACQUIRE_FRAMES * UP_INTERVALS) as u64;
        for (k, candidate) in self.candidates.iter_mut().enumerate() {
            if m < candidate.offset {
                continue;
            }
            let u = x - candidate.precoder.tail();
            candidate.precoder.accept(u, x);
            candidate.n += 1;
            if candidate.n <= acquire {
                continue;
            }
            candidate.decoder.feed(u * gain);
            let bits = candidate.decoder.take_bits();
            if self.chosen.is_none() {
                // What is left of B1u is ones: count them in the first bits
                // each reads, and keep what it read for the one chosen.
                for &bit in &bits {
                    if candidate.held.len() < JUDGED {
                        candidate.ones += usize::from(bit);
                    }
                }
                candidate.held.extend(bits);
                continue;
            }
            if self.chosen == Some(k) && !bits.is_empty() {
                if self.stream.bits.len().is_multiple_of(256) || self.stream.marks.is_empty() {
                    self.stream.marks.push((self.stream.bits.len(), now));
                }
                self.stream.bits.extend(bits);
            }
        }
        // The rest of B1u is ones: the place that reads most of them is the
        // one.
        if self.chosen.is_none() && self.candidates.iter().all(|c| c.held.len() >= JUDGED) {
            let best = self.candidates.iter().enumerate().max_by_key(|(_, c)| c.ones).map_or(0, |(k, _)| k);
            if std::env::var_os("REVIEW_V92").is_some() {
                eprintln!(
                    "V92 B1u ones by offset: {:?}, delay {}, fit {:.1} dB",
                    self.candidates.iter().map(|c| (c.offset, c.ones)).collect::<Vec<_>>(),
                    self.restorer.as_ref().map_or(0, |r| r.delay),
                    self.fit_db
                );
            }
            let held = std::mem::take(&mut self.candidates[best].held);
            self.stream.marks.push((0, now));
            self.stream.bits.extend(held);
            let ones = self.candidates[best].ones;
            let mut data = self.event(self.b1u_at, now, Layer::Data, "data");
            data = match self.bps {
                Some(bps) => data.summary(format!("data mode up at {bps} bit/s")).field("rate", format!("{bps} bit/s")),
                None => data.summary("data mode up".to_owned()),
            };
            data = data
                .field("B1u read as ones", format!("{} %", 100 * ones / JUDGED))
                .field("fit on B1u", format!("{:.1} dB", self.fit_db));
            self.stream.bps = self.bps;
            self.log.push(data);
            // Only the chosen one goes on.
            let keep = self.candidates.swap_remove(best);
            self.candidates = vec![keep];
            self.chosen = Some(0);
        }
        if let Some(data) = self.log.events.iter_mut().rev().find(|e| e.tag == "data" && e.from == self.who) {
            data.end = now;
        }
    }

    fn finish(&mut self, now: f64) {
        self.stream.marks.push((self.stream.bits.len(), now));
        let levels = std::mem::take(&mut self.levels);
        if let Some(data) = self.log.events.iter_mut().rev().find(|e| e.tag == "data") {
            *data = data.clone().pcm(&levels);
        }
    }
}

fn self_check(parameters: &Parameters) {
    let Ok(mut encoder) = datapump::v92::upstream::UpEncoder::new(parameters) else {
        eprintln!("V92 check: no encoder");
        return;
    };
    let mut symbols = encoder.b1u();
    for _ in 0..40_000 {
        symbols.push(encoder.next_symbol(|| true));
    }
    let Some(mut restorer) = Restorer::new(parameters) else { return };
    let mut precoder = Precoder::new(&parameters.filters);
    let mut decoder = UpDecoder::new(parameters);
    let (mut worst_x, mut worst_u) = (0.0f64, 0.0f64);
    let mut ones = 0;
    let mut bits = 0;
    for (n, symbol) in symbols.iter().enumerate() {
        let x = restorer.step(symbol.out);
        let Some(m) = n.checked_sub(restorer.delay) else { continue };
        let sent = &symbols[m];
        if m > 2000 {
            worst_x = worst_x.max((x - sent.x).abs());
        }
        let u = x - precoder.tail();
        precoder.accept(u, x);
        if m > 2000 {
            worst_u = worst_u.max((u - sent.u).abs());
        }
        if m >= 216 {
            decoder.feed(u * parameters.gain);
            for b in decoder.take_bits() {
                ones += usize::from(b);
                bits += 1;
            }
        }
    }
    eprintln!(
        "V92 check: rms out {:.4}, delay {}, worst x error {worst_x:.4}, worst u error {worst_u:.4}, {ones} ones of {bits}, gain {}, first outs {:?}",
        (symbols.iter().map(|s| s.out * s.out).sum::<f64>() / symbols.len() as f64).sqrt(),
        restorer.delay,
        parameters.gain,
        symbols.iter().take(6).map(|s| (s.u, s.x, s.v, s.out)).collect::<Vec<_>>()
    );
}

/// An equaliser from the line's levels to B1u as it was sent: the place in
/// `raw` B1u begins at, the taps that make it, and how well they do, in
/// decibels. The place is looked for where the receiver's own delay could
/// have put it, which is somewhere in the levels held from before.
fn fit_b1u(raw: &[f64], b1u: &[f64]) -> Option<(Vec<f64>, usize, f64)> {
    let mut best: Option<(f64, Vec<f64>, usize)> = None;
    let energy: f64 = b1u.iter().map(|b| b * b).sum();
    for start in EQ_TAPS / 2..RECENT + AHEAD {
        if start + b1u.len() + EQ_TAPS / 2 > raw.len() {
            break;
        }
        let row = |k: usize| &raw[start + k - EQ_TAPS / 2..start + k + EQ_TAPS / 2];
        let mut a = vec![Complex::ZERO; EQ_TAPS * EQ_TAPS];
        let mut b = vec![Complex::ZERO; EQ_TAPS];
        for (k, &target) in b1u.iter().enumerate() {
            let r = row(k);
            for i in 0..EQ_TAPS {
                b[i].re += r[i] * target;
                for j in 0..EQ_TAPS {
                    a[i * EQ_TAPS + j].re += r[i] * r[j];
                }
            }
        }
        let trace: f64 = (0..EQ_TAPS).map(|i| a[i * EQ_TAPS + i].re).sum();
        for i in 0..EQ_TAPS {
            a[i * EQ_TAPS + i].re += 1e-9 * trace;
        }
        let Some(w) = dsp::complex::solve_hermitian(&a, &b) else { continue };
        let w: Vec<f64> = w.iter().map(|c| c.re).collect();
        let left = energy - b.iter().zip(&w).map(|(b, w)| b.re * w).sum::<f64>();
        if best.as_ref().is_none_or(|(l, _, _)| left < *l) {
            best = Some((left, w, start));
        }
    }
    let (left, taps, start) = best?;
    if std::env::var_os("REVIEW_V92").is_some() {
        eprintln!("V92 B1u found {} symbols after the held levels began", start);
    }
    let db = 10.0 * (energy / left.max(1e-30)).log10();
    Some((taps, start, db))
}

/// One place B1u might begin, read through its own precoder and decoder.
struct Candidate {
    /// Symbols from the one after E2u.
    offset: i64,
    precoder: Precoder,
    decoder: UpDecoder,
    /// Symbols of B1u and data given so far.
    n: u64,
    ones: usize,
    held: Vec<bool>,
}

/// x(n) back from G v(n): the prefilter of 6.4.2 undone.
///
/// v(n) = sum z2(k) x(n-k) + sum p2(k) v(n-k), so q(n) = v(n) - sum p2(k)
/// v(n-k) is z2 applied to x, and x is q through a least-squares inverse of
/// z2, `delay` symbols late.
struct Restorer {
    gain: f64,
    p2: Vec<f64>,
    inverse: Vec<f64>,
    delay: usize,
    v: VecDeque<f64>,
    q: VecDeque<f64>,
}

impl Restorer {
    fn new(parameters: &Parameters) -> Option<Self> {
        let filters = &parameters.filters;
        if parameters.gain <= 0.0 || filters.z2.is_empty() {
            return None;
        }
        let z2 = &filters.z2;
        let (inverse, delay) = if z2.len() == 1 {
            (vec![1.0 / z2[0]], 0)
        } else {
            invert(z2)?
        };
        Some(Self {
            gain: parameters.gain,
            p2: filters.p2.clone(),
            inverse,
            delay,
            v: VecDeque::new(),
            q: VecDeque::new(),
        })
    }

    /// One equalised level in; x for the symbol `delay` back out.
    fn step(&mut self, level: f64) -> f64 {
        let v = level / self.gain;
        let feedback: f64 = self.p2.iter().zip(self.v.iter().rev()).map(|(p, past)| p * past).sum();
        self.v.push_back(v);
        while self.v.len() > self.p2.len() {
            self.v.pop_front();
        }
        self.q.push_back(v - feedback);
        while self.q.len() > self.inverse.len() {
            self.q.pop_front();
        }
        self.inverse.iter().zip(self.q.iter().rev()).map(|(w, q)| w * q).sum()
    }
}

/// A least-squares inverse of the FIR `z`, twice its length, and the delay it
/// brings: the one, of those tried, that leaves least.
fn invert(z: &[f64]) -> Option<(Vec<f64>, usize)> {
    let length = 2 * z.len();
    let correlation = |lag: usize| -> f64 { z.iter().zip(z.iter().skip(lag)).map(|(a, b)| a * b).sum() };
    let lags: Vec<f64> = (0..length).map(correlation).collect();
    let ridge = 1e-6 * lags[0];
    let mut a = vec![Complex::ZERO; length * length];
    for i in 0..length {
        for j in 0..length {
            a[i * length + j] = Complex::new(lags[i.abs_diff(j)] + if i == j { ridge } else { 0.0 }, 0.0);
        }
    }
    let total = length + z.len() - 1;
    let mut best: Option<(f64, Vec<f64>, usize)> = None;
    for delay in (0..total).step_by(4) {
        let b: Vec<Complex> = (0..length)
            .map(|i| Complex::new(if delay >= i && delay - i < z.len() { z[delay - i] } else { 0.0 }, 0.0))
            .collect();
        let Some(w) = dsp::complex::solve_hermitian(&a, &b) else { continue };
        let w: Vec<f64> = w.iter().map(|c| c.re).collect();
        // What is left of a unit impulse, ||z * w - delta||^2 = 1 - b . w.
        let left = 1.0 - b.iter().zip(&w).map(|(b, w)| b.re * w).sum::<f64>();
        if best.as_ref().is_none_or(|(l, _, _)| left < *l) {
            best = Some((left, w, delay));
        }
    }
    let (_, w, coarse) = best?;
    let mut best = (f64::INFINITY, w, coarse);
    for delay in coarse.saturating_sub(3)..(coarse + 4).min(total) {
        let b: Vec<Complex> = (0..length)
            .map(|i| Complex::new(if delay >= i && delay - i < z.len() { z[delay - i] } else { 0.0 }, 0.0))
            .collect();
        let Some(w) = dsp::complex::solve_hermitian(&a, &b) else { continue };
        let w: Vec<f64> = w.iter().map(|c| c.re).collect();
        let left = 1.0 - b.iter().zip(&w).map(|(b, w)| b.re * w).sum::<f64>();
        if left < best.0 {
            best = (left, w, delay);
        }
    }
    Some((best.1, best.2))
}

/// Which sample of each symbol a downstream recorded at `per` samples a
/// symbol was the codec's: the one on which Sd, {+W, 0, +W, -W, 0, -W}, is
/// most nearly itself.
fn sd_phase(samples: &[f32], first: usize, per: usize) -> usize {
    if per <= 1 {
        return 0;
    }
    const SD: [f64; 6] = [1.0, 0.0, 1.0, -1.0, 0.0, -1.0];
    let span = 6 * per;
    let window = 3 * span;
    let mut alike = 0;
    let mut n = first + window + span;
    while n + 4 * span < samples.len() {
        let (mut now, mut before, mut both) = (0.0f64, 0.0f64, 0.0f64);
        for i in n - window..n {
            let (a, b) = (f64::from(samples[i]), f64::from(samples[i - span]));
            now += a * a;
            before += b * b;
            both += a * b;
        }
        let found = now / window as f64 > 1e-4 && both / (now * before).sqrt().max(1e-30) > 0.95;
        alike = if found { alike + 1 } else { 0 };
        if alike >= 12 {
            let from = n;
            let mut best = (f64::NEG_INFINITY, 0);
            for phase in 0..per {
                let mut sums = [0.0f64; 6];
                for i in from..from + 4 * span {
                    if (i - first) % per != phase {
                        continue;
                    }
                    sums[((i - first) / per) % 6] += f64::from(samples[i]);
                }
                let total: f64 = sums.iter().map(|s| s * s).sum();
                if total <= 0.0 {
                    continue;
                }
                let norm = (total * 4.0).sqrt();
                let score = (0..6)
                    .map(|turn| (0..6).map(|k| sums[k] * SD[(k + turn) % 6]).sum::<f64>().abs() / norm)
                    .fold(0.0, f64::max);
                if score > best.0 {
                    best = (score, phase);
                }
            }
            return best.1;
        }
        n += per;
    }
    0
}

/// `samples` from `first` at 8000 a second, `lead` samples of silence put in
/// front.
fn to_network(samples: &[f32], first: usize, last: usize, fs: f64, lead: usize) -> Vec<f64> {
    let slice = &samples[first.min(samples.len())..last.min(samples.len())];
    if (fs - BAUD).abs() < 1.0 {
        return slice.iter().map(|&x| f64::from(x)).collect();
    }
    let mut resampler = Resampler::new(fs, BAUD);
    let mut out = Vec::with_capacity((slice.len() as f64 * BAUD / fs) as usize + 16);
    let mut scratch = Vec::new();
    for &x in std::iter::repeat_n(&0.0f32, lead).chain(slice) {
        scratch.clear();
        resampler.process(f64::from(x), &mut scratch);
        out.extend_from_slice(&scratch);
    }
    out
}

/// One reading of the call.
struct Reading {
    events: Vec<Event>,
    streams: Vec<Stream>,
    cpd: Option<Cpd>,
    upstream_rates: Option<u32>,
    downstream_bps: Option<u32>,
    reached_data: bool,
    trn1u_snr: f64,
    /// Bits read going up, which is the measure of a reading.
    up_bits: usize,
}

#[allow(clippy::too_many_arguments)]
fn read(
    analogue: (&[f32], End),
    digital: (&[f32], End),
    fs: f64,
    from: f64,
    until: f64,
    settled: &Settled,
    lead: usize,
    known_cpd: Option<Cpd>,
) -> Option<Reading> {
    let up_info = settled.info1a_pcm_up?;
    let law = if settled.info0d.is_some_and(|d| d.a_law) { Law::A } else { Law::Mu };
    let first = ((from + 0.01) * fs) as usize;
    let last = analogue.0.len().min(digital.0.len()).min((until * fs) as usize);
    if last <= first {
        return None;
    }
    let ratio = fs / BAUD;
    let per = (ratio.fract().abs() < 1e-9 && ratio >= 1.0).then_some(ratio as usize);
    let down_levels: Vec<f64> = match per {
        Some(per) => {
            let phase = sd_phase(digital.0, first, per);
            (first + phase..last).step_by(per).map(|i| f64::from(digital.0[i])).collect()
        }
        None => to_network(digital.0, first, last, fs, 0),
    };
    let up_levels = to_network(analogue.0, first, last, fs, lead);
    let start = first as f64 / fs;
    let mut shared = Shared::default();
    let mut down = Down::new(digital.1, law, up_info.uinfo);
    let mut up = Up::new(analogue.1, up_info.md_length, known_cpd);
    let symbols = down_levels.len().min(up_levels.len());
    for k in 0..symbols {
        let now = start + k as f64 / BAUD;
        down.feed(now, down_levels[k], &mut shared);
        up.feed(now, up_levels[k], &mut shared);
    }
    let end = start + symbols as f64 / BAUD;
    down.finish(end);
    up.finish(end);
    let reached_data = down.data_from.is_some() || up.stage == UpStage::Data;
    let up_bits = up.stream.bits.len();
    let mut events = up.log.events;
    events.extend(down.log.events);
    Some(Reading {
        events,
        streams: vec![up.stream, down.stream],
        cpd: shared.cpd,
        upstream_rates: shared.upstream_rates,
        downstream_bps: down.bps,
        reached_data,
        trn1u_snr: up.trn1u_snr.unwrap_or(f64::NEG_INFINITY),
        up_bits,
    })
}

/// Follow a V.92 call with PCM going up from the end of phase 2.
/// `analogue` and `digital` are the two modems' own transmissions.
pub fn follow(
    analogue: (&[f32], End),
    digital: (&[f32], End),
    fs: f64,
    settled: &Settled,
    until: f64,
) -> Option<(Vec<Event>, Outcome)> {
    settled.info1a_pcm_up?;
    let from = settled.ended?;
    // An upstream recorded at the network's own rate has its instants; one
    // recorded faster is tried at both halves of a symbol.
    let leads: &[usize] = if (fs - BAUD).abs() < 1.0 { &[0] } else { &[0, (fs / BAUD / 2.0).round() as usize] };
    let mut best: Option<Reading> = None;
    for &lead in leads {
        let Some(first) = read(analogue, digital, fs, from, until, settled, lead, None) else { continue };
        // The upstream's data fit wants the CPd before it has come down.
        let reading = match first.cpd.clone() {
            Some(cpd) => read(analogue, digital, fs, from, until, settled, lead, Some(cpd)).unwrap_or(first),
            None => first,
        };
        let better = best.as_ref().is_none_or(|b| {
            (reading.up_bits, (reading.trn1u_snr * 10.0) as i64) > (b.up_bits, (b.trn1u_snr * 10.0) as i64)
        });
        if better {
            best = Some(reading);
        }
    }
    let reading = best?;
    let outcome = Outcome {
        downstream_bps: reading.downstream_bps,
        upstream_rates: reading.upstream_rates.map(|r| r as u16),
        reached_data: reading.reached_data,
        streams: reading.streams,
    };
    Some((reading.events, outcome))
}
