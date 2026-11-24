//! Call set-up, heard and not taken part in: the answer tone, the calling
//! tones, V.8 and V.8 bis.
//!
//! Each of these is a listener the modem already has, taken out of the state
//! machine that would have answered what it heard. They are all run over one
//! channel at once, because on a tap nothing says in advance which of them
//! is due.

use datapump::bell103::Bell103Rx;
use datapump::v8::{HIGH, LOW};
use datapump::v8bis::{self, Ear, Side};
use datapump::v21;
use datapump::v92::anspcm::{AnswerSignal, AnspcmWatch};
use dsp::{ReversalDetector, ToneDetector};
use ec::hdlc::{Decoder as Hdlc, Fcs};
use v8::quick::{BitWatcher, Channel as QcChannel, Field, Qc};
use v8::{Decoder as V8Decoder, Heard, Menu};

use crate::model::{End, Event, Layer};

/// Something found on one channel, with how loud it was there: on a
/// recording with an end a channel, each end's signals are on the other's
/// channel too, as echo, and the level is what tells the two apart.
#[derive(Debug, Clone)]
pub struct Found {
    pub event: Event,
    /// Amplitude, on the scale of the samples.
    pub level: f64,
}

/// A condition that has to stand for a while to count, and may blink.
#[derive(Debug, Clone)]
struct Span {
    /// Samples it may be false for without having ended.
    blink: usize,
    start: Option<usize>,
    last: usize,
    peak: f64,
}

impl Span {
    fn new(blink_secs: f64, fs: f64) -> Self {
        Self { blink: (blink_secs * fs) as usize, start: None, last: 0, peak: 0.0 }
    }

    /// The span that has just ended, as (first, last, peak level).
    fn feed(&mut self, n: usize, on: bool, level: f64) -> Option<(usize, usize, f64)> {
        if on {
            if self.start.is_none() {
                self.start = Some(n);
                self.peak = 0.0;
            }
            self.last = n;
            self.peak = self.peak.max(level);
            return None;
        }
        let start = self.start?;
        if n - self.last <= self.blink {
            return None;
        }
        self.start = None;
        Some((start, self.last, self.peak))
    }

    fn finish(&mut self) -> Option<(usize, usize, f64)> {
        self.start.take().map(|start| (start, self.last, self.peak))
    }
}

/// One plain tone: most of what is on the line, for long enough.
struct Plain {
    hz: f64,
    tag: &'static str,
    what: &'static str,
    from: End,
    least: f64,
    detector: ToneDetector,
    span: Span,
    /// Where the tone's phasor has stood, every few milliseconds.
    phasors: Vec<(f32, f32)>,
}

/// Samples between two readings of a phasor: 2.5 ms.
fn phasor_step(fs: f64) -> usize {
    ((fs / 400.0) as usize).max(1)
}

impl Plain {
    fn new(hz: f64, tag: &'static str, what: &'static str, from: End, least: f64, fs: f64) -> Self {
        Self {
            hz,
            tag,
            what,
            from,
            least,
            detector: ToneDetector::new(hz, 40.0, fs),
            span: Span::new(0.03, fs),
            phasors: Vec::new(),
        }
    }

    fn feed(&mut self, n: usize, x: f64, rms: f64, fs: f64, out: &mut Vec<Found>) {
        self.detector.feed(x);
        let amplitude = self.detector.amplitude();
        // A sine alone reads 1.41 of the line's root mean square.
        let on = rms > 1e-3 && amplitude > 1.2 * rms;
        if on && self.span.start.is_none() {
            self.phasors.clear();
        }
        if on && n.is_multiple_of(phasor_step(fs)) {
            let (re, im) = self.detector.phasor();
            self.phasors.push((re as f32, im as f32));
        }
        if let Some(span) = self.span.feed(n, on, amplitude) {
            self.emit(span, fs, out);
        }
    }

    fn finish(&mut self, fs: f64, out: &mut Vec<Found>) {
        if let Some(span) = self.span.finish() {
            self.emit(span, fs, out);
        }
    }

    fn emit(&self, (first, last, peak): (usize, usize, f64), fs: f64, out: &mut Vec<Found>) {
        let (start, end) = (first as f64 / fs, last as f64 / fs);
        if end - start < self.least {
            return;
        }
        let event = Event::new(start, end, self.from, Layer::Tone, self.tag)
            .summary(format!("{}, {:.0} Hz for {:.2} s", self.what, self.hz, end - start))
            .field("frequency", format!("{:.0} Hz", self.hz))
            .field("level", db(peak))
            .phasor(&self.phasors)
            .band(self.hz - 60.0, self.hz + 60.0);
        out.push(Found { event, level: peak });
    }
}

/// Amplitude of a sine as dB below full scale.
pub fn db(amplitude: f64) -> String {
    format!("{:.1} dBFS", 20.0 * (amplitude + 1e-9).log10())
}

/// The answer tone: which of the three it was, and its reversals.
struct Answer {
    watch: AnspcmWatch,
    reversals: ReversalDetector,
    span: Span,
    kind: Option<AnswerSignal>,
    turned: Vec<usize>,
    deepest: f64,
    probe: ToneDetector,
    phasors: Vec<(f32, f32)>,
}

impl Answer {
    fn new(fs: f64) -> Self {
        Self {
            watch: AnspcmWatch::new(fs),
            reversals: ReversalDetector::new(2100.0, 60.0, 0.004, fs),
            span: Span::new(0.06, fs),
            kind: None,
            turned: Vec::new(),
            deepest: 0.0,
            probe: ToneDetector::new(2100.0, 80.0, fs),
            phasors: Vec::new(),
        }
    }

    fn feed(&mut self, n: usize, x: f64, fs: f64, out: &mut Vec<Found>) {
        self.watch.feed(x);
        self.probe.feed(x);
        let turned = self.reversals.feed(x);
        let tone = self.watch.tone();
        let on = tone.present();
        if on && self.span.start.is_none() {
            self.phasors.clear();
        }
        if on && n.is_multiple_of(phasor_step(fs)) {
            let (re, im) = self.probe.phasor();
            self.phasors.push((re as f32, im as f32));
        }
        if on {
            if let Some(kind) = self.watch.signal() {
                self.kind = Some(kind);
            }
            self.deepest = self.deepest.max(tone.depth());
            if turned && self.span.start.is_some() {
                self.turned.push(n.saturating_sub(self.reversals.latency() as usize));
            }
        }
        let level = tone.amplitude();
        if let Some(span) = self.span.feed(n, on, level) {
            self.emit(span, fs, out);
        }
    }

    fn finish(&mut self, fs: f64, out: &mut Vec<Found>) {
        if let Some(span) = self.span.finish() {
            self.emit(span, fs, out);
        }
    }

    fn emit(&mut self, (first, last, peak): (usize, usize, f64), fs: f64, out: &mut Vec<Found>) {
        let kind = self.kind.take();
        let turned = std::mem::take(&mut self.turned);
        let deepest = std::mem::replace(&mut self.deepest, 0.0);
        self.watch.reset();
        self.reversals.restart();
        let (start, end) = (first as f64 / fs, last as f64 / fs);
        if end - start < 0.2 {
            return;
        }
        let (tag, what) = match kind {
            Some(AnswerSignal::Ansam) => ("ANSam", "V.8 answer tone, 2100 Hz with 15 Hz on it"),
            Some(AnswerSignal::Anspcm) => ("ANSpcm", "V.92 quick-connect answer tone"),
            Some(AnswerSignal::Ans) => ("ANS", "V.25 answer tone, 2100 Hz"),
            None => ("2100 Hz", "2100 Hz, too short to say which answer tone"),
        };
        let gaps: Vec<f64> = turned.windows(2).map(|w| (w[1] - w[0]) as f64 / fs).collect();
        let every = if gaps.is_empty() { None } else { Some(gaps.iter().sum::<f64>() / gaps.len() as f64) };
        let mut summary = format!("{what}, {:.2} s", end - start);
        if !turned.is_empty() {
            summary.push_str(&format!(", {} phase reversals", turned.len()));
        }
        let mut event = Event::new(start, end, End::Answerer, Layer::Tone, tag)
            .summary(summary)
            .field("duration", format!("{:.2} s", end - start))
            .field("level", db(peak))
            .field("15 Hz modulation", format!("{:.0} % deep", 100.0 * deepest))
            .field("phase reversals", turned.len().to_string())
            .phasor(&self.phasors)
            .band(2040.0, 2160.0);
        if let Some(every) = every {
            event = event.field("reversal interval", format!("{:.0} ms", 1000.0 * every));
        }
        if let Some(qts) = self.watch.qts_reversal() {
            event = event.field("QTS reversal", format!("{:.3} s", qts / fs));
        }
        out.push(Found { event, level: peak });
    }
}

/// What a menu says, as fields.
pub fn menu_fields(menu: &Menu) -> Vec<(String, String)> {
    let mut fields = vec![("call function".to_owned(), words(&format!("{:?}", menu.function)))];
    let modulations: Vec<&str> = menu.modulations.iter().map(|m| m.name()).collect();
    fields.push((
        "modulations".to_owned(),
        if modulations.is_empty() { "none".to_owned() } else { modulations.join(", ") },
    ));
    fields.push(("protocol".to_owned(), match menu.protocol {
        v8::Protocol::Lapm => "LAPM (V.42)".to_owned(),
        v8::Protocol::Unstated => "not stated".to_owned(),
        v8::Protocol::Extended => "by extension octet".to_owned(),
    }));
    if let Some(pcm) = menu.pcm {
        let mut can = Vec::new();
        if pcm.analogue {
            can.push("V.90/V.92 analogue");
        }
        if pcm.digital {
            can.push("V.90/V.92 digital");
        }
        if pcm.v91 {
            can.push("V.91");
        }
        fields.push(("PCM modem".to_owned(), if can.is_empty() { "none".to_owned() } else { can.join(", ") }));
    }
    if let Some(access) = menu.access {
        let mut says = Vec::new();
        if access.digital {
            says.push("digital network connection");
        }
        if access.call_cellular {
            says.push("calling end cellular");
        }
        if access.answer_cellular {
            says.push("answering end cellular");
        }
        fields.push(("network access".to_owned(), if says.is_empty() { "analogue".to_owned() } else { says.join(", ") }));
    }
    fields
}

/// "TransmitFax" as "transmit fax".
fn words(camel: &str) -> String {
    let mut out = String::new();
    for c in camel.chars() {
        if c.is_ascii_uppercase() && !out.is_empty() {
            out.push(' ');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

fn menu_line(menu: &Menu) -> String {
    let fields = menu_fields(menu);
    let mut line = format!("{}: {}", fields[0].1, fields[1].1);
    if menu.protocol == v8::Protocol::Lapm {
        line.push_str(", LAPM");
    }
    if let Some(pcm) = fields.iter().find(|(n, _)| n == "PCM modem") {
        line.push_str(&format!(", PCM {}", pcm.1));
    }
    line
}

fn hex(octets: &[u8]) -> String {
    octets.iter().map(|o| format!("{o:02X}")).collect::<Vec<_>>().join(" ")
}

/// One of V.8's two channels, and a run of the same sequence on it.
struct V8Ear {
    answerer: bool,
    rx: Bell103Rx,
    decoder: V8Decoder,
    run: Option<Run>,
    /// The discriminator at the middle of each bit, and when.
    margins: Vec<(f64, f32)>,
}

/// Keep the last few thousand bits of an FSK channel.
fn keep(margins: &mut Vec<(f64, f32)>, now: f64, reading: Option<f64>) {
    if let Some(m) = reading {
        margins.push((now, m as f32));
        if margins.len() > 8000 {
            margins.drain(..4000);
        }
    }
}

/// The readings between two times.
fn between(margins: &[(f64, f32)], start: f64, end: f64) -> Vec<f32> {
    margins.iter().filter(|(t, _)| *t >= start && *t <= end).map(|m| m.1).collect()
}

struct Run {
    heard: Heard,
    octets: Vec<u8>,
    first: f64,
    last: f64,
    times: usize,
    level: f64,
}

impl V8Ear {
    fn new(answerer: bool, fs: f64) -> Self {
        let (space, mark) = if answerer { HIGH } else { LOW };
        Self {
            answerer,
            rx: Bell103Rx::with_tones(space, mark, fs),
            decoder: V8Decoder::new(),
            run: None,
            margins: Vec::new(),
        }
    }

    fn feed(&mut self, n: usize, x: f64, fs: f64, out: &mut Vec<Found>) {
        let now = n as f64 / fs;
        let octet = self.rx.feed(x);
        let reading = self.rx.take_symbol();
        keep(&mut self.margins, now, reading);
        if let Some(octet) = octet
            && let Some(heard) = self.decoder.feed(octet)
        {
            let heard = if self.answerer { V8Decoder::heard_as_jm(heard) } else { heard };
            // CM belongs to the low channel and JM to the high. A CM on the
            // high one has been renamed already; a CI or CJ there is noise.
            if self.answerer && !matches!(heard, Heard::Jm(_)) {
                return;
            }
            let octets = match heard {
                Heard::Cj => vec![0, 0, 0],
                _ => self.decoder.sequence().to_vec(),
            };
            // Ten bits an octet at 300 a second, and the sync before them.
            let took = (octets.len() + 2) as f64 * 10.0 / 300.0;
            let level = self.rx.amplitude();
            // CI comes in bursts with silence between; the menus back to back.
            let gap = if matches!(heard, Heard::Ci(_)) { 2.5 } else { 0.5 };
            match &mut self.run {
                Some(run) if run.heard == heard && now - run.last < gap => {
                    run.last = now;
                    run.times += 1;
                    run.level = run.level.max(level);
                }
                _ => {
                    self.flush(out);
                    self.run = Some(Run { heard, octets, first: now - took, last: now, times: 1, level });
                }
            }
        }
        if self.run.as_ref().is_some_and(|run| now - run.last > 3.0) {
            self.flush(out);
        }
    }

    fn flush(&mut self, out: &mut Vec<Found>) {
        let Some(run) = self.run.take() else { return };
        let (low, high) = if self.answerer { (1650.0, 1850.0) } else { (980.0, 1180.0) };
        let from = if self.answerer { End::Answerer } else { End::Caller };
        let (tag, summary, fields) = match &run.heard {
            Heard::Ci(function) => {
                let what = words(&format!("{function:?}"));
                ("CI", format!("call indicator: {what}"), vec![("call function".to_owned(), what)])
            }
            Heard::Cm(menu) => ("CM", menu_line(menu), menu_fields(menu)),
            Heard::Jm(menu) => ("JM", menu_line(menu), menu_fields(menu)),
            Heard::Cj => ("CJ", "CM terminator: the joint menu is taken".to_owned(), Vec::new()),
        };
        // A menu is only believed once it has come the same twice running,
        // which is the rule the modems themselves go by (V.8 7.3), and a CI
        // is sent three times at the least. The demodulator turns whatever
        // is in its band into characters, training signals included, and
        // sooner or later some of them make a sequence: once.
        if !matches!(run.heard, Heard::Cj) && run.times < 2 {
            return;
        }
        let mut event = Event::new(run.first.max(0.0), run.last, from, Layer::V8, tag)
            .summary(summary)
            .fields(fields)
            .field("sent", format!("{} times", run.times))
            .field("octets", hex(&run.octets))
            .field("channel", format!("V.21 {low:.0}/{high:.0} Hz, 300 bit/s"))
            .octets(&run.octets)
            .fsk(&between(&self.margins, run.first - 0.02, run.last + 0.01))
            .band(low - 60.0, high + 60.0);
        if matches!(run.heard, Heard::Cj) {
            event.fields.retain(|(name, _)| name != "sent");
        }
        out.push(Found { event, level: run.level });
    }
}

/// V.92's quick-connect sequences, which are bits and not characters.
struct QcEar {
    channel: QcChannel,
    rx: v21::Receiver,
    watcher: BitWatcher,
    last: Option<(Qc, f64)>,
    margins: Vec<(f64, f32)>,
}

impl QcEar {
    fn new(channel: QcChannel, fs: f64) -> Self {
        let tones = if channel == QcChannel::High { HIGH } else { LOW };
        Self { channel, rx: v21::Receiver::on(tones, fs), watcher: BitWatcher::new(), last: None, margins: Vec::new() }
    }

    fn feed(&mut self, n: usize, x: f64, fs: f64, out: &mut Vec<Found>) {
        let now = n as f64 / fs;
        let bit = self.rx.feed(x);
        let reading = self.rx.take_symbol();
        keep(&mut self.margins, now, reading);
        let Some(bit) = bit else { return };
        let Some(qc) = self.watcher.feed(bit) else { return };
        if qc.kind.channel() != self.channel || !qc.consistent() {
            return;
        }
        // The repeat of one already reported.
        if self.last.is_some_and(|(was, at)| was == qc && now - at < 1.0) {
            return;
        }
        self.last = Some((qc, now));
        let bits = qc.bits().len() as f64;
        let (low, high) = if self.channel == QcChannel::High { (1650.0, 1850.0) } else { (980.0, 1180.0) };
        let from = if self.channel == QcChannel::High { End::Answerer } else { End::Caller };
        let field = match qc.field {
            Field::Uqts(uqts) => ("U_QTS".to_owned(), format!("{uqts:?}")),
            Field::Level(level) => ("ANSpcm level".to_owned(), format!("{:.0} dBm0", level.dbm0())),
        };
        let asks = if qc.kind.acknowledgement() { "acknowledges a quick connect" } else { "asks for a quick connect" };
        let who = if qc.kind.digital() { "digital modem" } else { "analogue modem" };
        let event = Event::new((now - bits / 300.0).max(0.0), now, from, Layer::V8, qc.kind.name())
            .summary(format!("{who} {asks}{}", if qc.lapm { ", LAPM" } else { "" }))
            .field("sent by", who)
            .field("LAPM without detection", if qc.lapm { "yes" } else { "no" })
            .field(field.0, field.1)
            .field("octet", format!("{:02X}", qc.octet()))
            .octets(&[qc.octet()])
            .fsk(&between(&self.margins, now - bits / 300.0 - 0.01, now))
            .band(low - 60.0, high + 60.0);
        out.push(Found { event, level: self.rx.band_level() });
    }
}

/// One of V.8 bis's two message channels.
struct BisEar {
    side: Side,
    rx: v21::Receiver,
    hdlc: Hdlc,
    power: f64,
    alpha: f64,
    carrier: Option<usize>,
    quiet: usize,
    hold: usize,
    peak: f64,
    margins: Vec<(f64, f32)>,
}

impl BisEar {
    fn new(side: Side, fs: f64) -> Self {
        let channel = if side == Side::Initiating { v21::CHANNEL_1 } else { v21::CHANNEL_2 };
        Self {
            side,
            rx: v21::Receiver::on(channel, fs),
            hdlc: Hdlc::new(Fcs::Bits16),
            power: 0.0,
            alpha: 1.0 - (-1.0 / (0.008 * fs)).exp(),
            carrier: None,
            quiet: 0,
            hold: (0.012 * fs) as usize,
            peak: 0.0,
            margins: Vec::new(),
        }
    }

    fn feed(&mut self, n: usize, x: f64, fs: f64, out: &mut Vec<Found>) {
        let bit = self.rx.feed(x);
        let reading = self.rx.take_symbol();
        keep(&mut self.margins, n as f64 / fs, reading);
        self.power += self.alpha * (x * x - self.power);
        // The same test of a carrier the modem's own V.8 bis makes.
        let band = self.power > 1e-8 && self.rx.band_level() > 0.38 * self.power.sqrt();
        if band {
            self.quiet = 0;
            if self.carrier.is_none() {
                self.carrier = Some(n);
                self.peak = 0.0;
            }
            self.peak = self.peak.max(self.rx.band_level());
        } else if self.carrier.is_some() {
            self.quiet += 1;
            if self.quiet > self.hold {
                self.hdlc = Hdlc::new(Fcs::Bits16);
                self.carrier = None;
            }
        }
        let Some(since) = self.carrier else { return };
        let Some(bit) = bit else { return };
        let Some(frame) = self.hdlc.feed(bit) else { return };
        let Some(octets) = v8bis::information(frame, &self.hdlc) else { return };
        let (start, end) = (since as f64 / fs, n as f64 / fs);
        let fsk = between(&self.margins, start, end);
        // The next message on this carrier starts here.
        self.carrier = Some(n);
        let (low, high) = if self.side == Side::Initiating { (980.0, 1180.0) } else { (1650.0, 1850.0) };
        let station = if self.side == Side::Initiating { "initiating" } else { "responding" };
        let channel = format!("V.21 {low:.0}/{high:.0} Hz, 300 bit/s, HDLC");

        // T.30 uses the same channel and the same framing: an address of all
        // ones and a control field that says whether more frames follow.
        if self.side == Side::Responding && octets.len() >= 3 && octets[0] == 0xFF && (octets[1] & 0xEF) == 0x03 {
            let event = Event::new(start, end, End::Line, Layer::Fax, "T.30")
                .summary(format!("T.30 frame, control field {:02X}, {} octets", octets[2], octets.len()))
                .field("facsimile control field", format!("{:02X}", octets[2]))
                .field("final frame", if octets[1] & 0x10 != 0 { "yes" } else { "no" })
                .field("octets", hex(&octets))
                .field("channel", channel)
                .octets(&octets)
                .fsk(&fsk)
                .band(low - 60.0, high + 60.0);
            out.push(Found { event, level: self.peak });
            return;
        }
        let Some(message) = v8::bis::Message::decode(&octets) else { return };
        let mut event = Event::new(start, end, End::Line, Layer::V8bis, message.kind.name())
            .field("station", station)
            .field("revision", message.revision.to_string());
        let summary = if message.kind.has_parameters() {
            let can = v8bis::capabilities(&message);
            event = event.field("capabilities", can.clone());
            let id = message.identify();
            event = event.field("V.8 after the transaction", if id.v8 { "yes" } else { "no" });
            if !message.non_standard.is_empty() {
                event = event.field("non-standard field", hex(&message.non_standard));
            }
            format!("{} from the {station} station: {can}", message.kind.name())
        } else {
            format!("{} from the {station} station", message.kind.name())
        };
        let event = event
            .summary(summary)
            .field("octets", hex(&octets))
            .field("channel", channel)
            .octets(&octets)
            .fsk(&fsk)
            .band(low - 60.0, high + 60.0);
        out.push(Found { event, level: self.peak });
    }
}

/// Everything of call set-up there is on one channel.
pub fn listen(samples: &[f32], fs: f64) -> Vec<Found> {
    let mut out = Vec::new();
    let mut answer = Answer::new(fs);
    let mut plain = [
        Plain::new(1100.0, "CNG", "fax calling tone", End::Caller, 0.3, fs),
        Plain::new(1300.0, "CT", "V.25 calling tone", End::Caller, 0.3, fs),
    ];
    let mut v8 = [V8Ear::new(false, fs), V8Ear::new(true, fs)];
    let mut qc = [QcEar::new(QcChannel::Low, fs), QcEar::new(QcChannel::High, fs)];
    let mut bis_tones = Ear::new(fs);
    let mut bis = [BisEar::new(Side::Initiating, fs), BisEar::new(Side::Responding, fs)];
    let alpha = 1.0 - (-1.0 / (0.02 * fs)).exp();
    let mut power = 0.0f64;

    for (n, &sample) in samples.iter().enumerate() {
        let x = sample as f64;
        power += alpha * (x * x - power);
        let rms = power.sqrt();
        answer.feed(n, x, fs, &mut out);
        for tone in &mut plain {
            tone.feed(n, x, rms, fs, &mut out);
        }
        for ear in &mut v8 {
            ear.feed(n, x, fs, &mut out);
        }
        for ear in &mut qc {
            ear.feed(n, x, fs, &mut out);
        }
        if let Some((signal, side)) = bis_tones.feed(x) {
            let now = n as f64 / fs;
            let (a, b) = side.pair();
            let station = if side == Side::Initiating { "initiating" } else { "responding" };
            let event = Event::new((now - 0.45).max(0.0), now + 0.07, End::Line, Layer::V8bis, signal.name())
                .summary(format!("V.8 bis signal {} from the {station} station", signal.name()))
                .field("station", station)
                .field("first segment", format!("{a:.0} + {b:.0} Hz"))
                .field("second segment", format!("{:.0} Hz", signal.segment_2()))
                .band(signal.segment_2().min(a) - 60.0, b + 60.0);
            out.push(Found { event, level: rms * std::f64::consts::SQRT_2 });
        }
        for ear in &mut bis {
            ear.feed(n, x, fs, &mut out);
        }
    }
    answer.finish(fs, &mut out);
    for tone in &mut plain {
        tone.finish(fs, &mut out);
    }
    for ear in &mut v8 {
        ear.flush(&mut out);
    }
    out
}
