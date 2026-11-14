//! What can be read of an echo-cancelled call off a recording of one tap.
//!
//! V.32 and V.34 put both directions on the same band and tell them apart by
//! each end subtracting what it sent itself. A recording made at one point on
//! the pair has the sum and neither end's transmission to subtract, so the
//! data cannot be had from it by anybody. What can be had is every part of
//! the start-up where one modem is alone on the line, and both
//! recommendations arrange for exactly the parts worth seeing to be that:
//! each end's training signal, sent while the other listens.
//!
//! So these follow a call as far as that goes. V.32: the answering modem's
//! S, S-bar and TRN and the rate signal after it, then the calling modem's.
//! V.34: the INFO sequences of phase 2, which are on a carrier each, and then
//! each modem's S, PP and TRN of phase 3. After that both ends send at once,
//! and the phase says so rather than drawing the sum as though it were a
//! constellation.

use std::collections::VecDeque;

use datapump::v32::startup::{RateDetector, describe_sequence, is_rate_signal};
use datapump::v32::{Mode, Receiver as V32Rx};
use datapump::v34::dpsk::{Receiver as InfoRx, Side};
use datapump::v34::info::{Info, Info1c, SymbolRate};
use datapump::v34::qam::Band;
use datapump::v34::receiver::{Heard, Receiver as V34Rx, Reference};
use datapump::v34::signals::{J_FOUR, J_SIXTEEN, Reader, Size};

/// The last phase of either: nothing more to be read.
const BOTH: &str = "both ends sending on one band: a single tap cannot tell them apart";

/// Where a V.32 call on one tap has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum V32Step {
    /// Listening for the answering modem's conditioning signal.
    Answerer,
    /// Its rate signal has been read; this many samples until the calling
    /// modem's S is due.
    Gap(usize),
    /// Listening for the calling modem's.
    Caller,
    Both,
}

/// A V.32 or V.32bis call followed from one tap.
pub struct V32Tap {
    fs: f64,
    /// A receiver as the calling end has it, which is the one that hears the
    /// answering modem; and the other way about.
    call_end: V32Rx,
    answer_end: V32Rx,
    call_rates: RateDetector,
    answer_rates: RateDetector,
    step: V32Step,
    trained_said: bool,
    said: Vec<(&'static str, String)>,
    notes: Vec<String>,
}

impl V32Tap {
    pub fn new(fs: f64) -> Self {
        let mut call_end = V32Rx::new(Mode::Call, fs);
        let mut answer_end = V32Rx::new(Mode::Answer, fs);
        call_end.hunt();
        // Idle, but kept fed: the calling modem's S is found in samples the
        // receiver already holds.
        answer_end.idle();
        Self {
            fs,
            call_end,
            answer_end,
            call_rates: RateDetector::new(),
            answer_rates: RateDetector::new(),
            step: V32Step::Answerer,
            trained_said: false,
            said: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn feed(&mut self, x: f64) {
        if self.step == V32Step::Both {
            return;
        }
        self.call_end.feed(x);
        self.answer_end.feed(x);
        match self.step {
            V32Step::Answerer => {
                if !self.trained_said && self.call_end.trained_via().is_some() {
                    self.trained_said = true;
                    self.notes.push(format!(
                        "the answering modem's TRN trained the receiver to {:.1} dB",
                        self.call_end.trained_snr_db()
                    ));
                }
                for bit in self.call_end.take_bits() {
                    if let Some(s) = self.call_rates.feed(bit)
                        && is_rate_signal(s)
                    {
                        self.said.push(("answerer R1", describe_sequence(s)));
                        self.notes.push(format!("the answering modem's R1: {}", describe_sequence(s)));
                        // The calling modem answers R1 with its own S about a
                        // tenth of a second on; hunting sooner would find the
                        // end of this one.
                        self.step = V32Step::Gap((0.1 * self.fs) as usize);
                        self.trained_said = false;
                        break;
                    }
                }
            }
            V32Step::Gap(left) => {
                self.step = if left == 0 {
                    self.call_end.idle();
                    self.answer_end.hunt();
                    V32Step::Caller
                } else {
                    V32Step::Gap(left - 1)
                };
            }
            V32Step::Caller => {
                if !self.trained_said && self.answer_end.trained_via().is_some() {
                    self.trained_said = true;
                    self.notes.push(format!(
                        "the calling modem's TRN trained the receiver to {:.1} dB",
                        self.answer_end.trained_snr_db()
                    ));
                }
                for bit in self.answer_end.take_bits() {
                    if let Some(s) = self.answer_rates.feed(bit)
                        && is_rate_signal(s)
                    {
                        self.said.push(("caller R2", describe_sequence(s)));
                        self.notes.push(format!("the calling modem's R2: {}", describe_sequence(s)));
                        self.notes.push(BOTH.to_owned());
                        self.step = V32Step::Both;
                        break;
                    }
                }
            }
            V32Step::Both => {}
        }
    }

    /// The receiver being watched, if it is making symbols worth drawing.
    fn watched(&self) -> Option<&V32Rx> {
        let rx = match self.step {
            V32Step::Answerer | V32Step::Gap(_) => &self.call_end,
            V32Step::Caller => &self.answer_end,
            V32Step::Both => return None,
        };
        (rx.is_tracking() && !rx.is_lost()).then_some(rx)
    }

    pub fn point(&self) -> Option<(f32, f32)> {
        self.watched().map(|rx| {
            let (i, q) = rx.constellation_point();
            (i as f32, q as f32)
        })
    }

    pub fn snr_db(&self) -> Option<f32> {
        self.watched().map(|rx| rx.snr_db() as f32)
    }

    pub fn training(&self) -> bool {
        self.watched().is_some()
    }

    pub fn phase(&self) -> &'static str {
        match (self.step, self.training()) {
            (V32Step::Answerer | V32Step::Gap(_), true) => "the answering modem's TRN, alone on the line",
            (V32Step::Answerer, false) => "waiting for the answering modem's S",
            (V32Step::Gap(_), false) => "the answering modem's rate signal read",
            (V32Step::Caller, true) => "the calling modem's TRN, alone on the line",
            (V32Step::Caller, false) => "waiting for the calling modem's S",
            (V32Step::Both, _) => BOTH,
        }
    }

    pub fn said(&self) -> &[(&'static str, String)] {
        &self.said
    }

    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }
}

/// One modem's phase 3, heard by a receiver on its band: S and S-bar found,
/// trained on PP and TRN, and TRN read until it gives way to J.
struct Third {
    rx: V34Rx,
    reader: Reader,
    /// Whose scrambler the signal was made with.
    far: Mode,
    name: &'static str,
    trained: bool,
    in_trn: bool,
    /// Bits read so far, and the last forty-eight of them for J to be found
    /// in: three repeats of its sixteen.
    read: usize,
    tail: VecDeque<bool>,
    j: bool,
    point: Option<(f32, f32)>,
    /// Samples fed, for giving up on a signal that never comes.
    fed: usize,
}

impl Third {
    fn new(band: Band, far: Mode, name: &'static str, fs: f64) -> Self {
        let mut rx = V34Rx::new(band, fs);
        rx.hunt();
        Self {
            rx,
            reader: Reader::new(far),
            far,
            name,
            trained: false,
            in_trn: true,
            read: 0,
            tail: VecDeque::with_capacity(48),
            j: false,
            point: None,
            fed: 0,
        }
    }

    fn feed(&mut self, x: f64, said: &mut Vec<(&'static str, String)>, notes: &mut Vec<String>) {
        self.fed += 1;
        self.rx.feed(x);
        while let Some(heard) = self.rx.heard() {
            match heard {
                Heard::S => notes.push(format!("the {} modem's S", self.name)),
                Heard::Reversal { at } => self.rx.train(Reference::PpThenTrn, self.far, at),
                Heard::Trained { snr_db } => {
                    self.trained = true;
                    notes.push(format!(
                        "the {} modem's PP and TRN trained the receiver to {snr_db:.1} dB",
                        self.name
                    ));
                }
                Heard::Untrained => {
                    notes.push(format!("the {} modem's PP was not where its S-bar said", self.name));
                    self.rx.hunt();
                }
                Heard::Symbol(symbol) => {
                    self.point = Some((symbol.point.re as f32, symbol.point.im as f32));
                    // TRN descrambles to ones. The first few dozen bits are
                    // the scrambler filling, and after that the first bit
                    // that is not a one is the start of J.
                    if self.in_trn {
                        let got = self.reader.trn(symbol.decided, Size::Four);
                        if got.iter().all(|b| *b) || self.read < 46 {
                            self.read += got.len();
                            continue;
                        }
                        self.in_trn = false;
                    }
                    for bit in self.reader.differential(symbol.decided, Size::Four) {
                        if self.tail.len() == 48 {
                            self.tail.pop_front();
                        }
                        self.tail.push_back(bit);
                    }
                    if !self.j && self.tail.len() == 48 {
                        for (j, asks) in [(J_FOUR, "4"), (J_SIXTEEN, "16")] {
                            // Rust note: `make_contiguous` would need `&mut`;
                            // comparing by position does the same with none.
                            if self.tail.iter().enumerate().all(|(i, b)| *b == j[i % 16]) {
                                self.j = true;
                                let text = format!("asks for {asks}-point training in phase 4");
                                notes.push(format!("the {} modem's J {text}", self.name));
                                said.push((if self.far == Mode::Answer { "answerer J" } else { "caller J" }, text));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Over: J has been read, the signal was lost after training, or it never
    /// came in four seconds.
    fn over(&self, fs: f64) -> bool {
        self.j || (self.trained && self.rx.is_lost()) || (!self.trained && self.fed > (4.0 * fs) as usize)
    }

    fn drawing(&self) -> bool {
        self.trained && !self.rx.is_lost()
    }
}

enum V34Step {
    /// V.8 and phase 2: the INFO sequences, on a carrier each.
    Second,
    Answerer(Box<Third>),
    Caller(Box<Third>),
    Both,
}

/// A V.34 call followed from one tap.
pub struct V34Tap {
    fs: f64,
    call_info: InfoRx,
    answer_info: InfoRx,
    info1c: Option<Info1c>,
    /// The call modem's band for its own phase 3, from INFO1a.
    call_band: Option<Band>,
    step: V34Step,
    said: Vec<(&'static str, String)>,
    notes: Vec<String>,
}

impl V34Tap {
    pub fn new(fs: f64) -> Self {
        Self {
            fs,
            call_info: InfoRx::new(Side::Call, fs),
            answer_info: InfoRx::new(Side::Answer, fs),
            info1c: None,
            call_band: None,
            step: V34Step::Second,
            said: Vec::new(),
            notes: Vec::new(),
        }
    }

    pub fn feed(&mut self, x: f64) {
        match &mut self.step {
            V34Step::Second => {
                if let Some(info) = self.call_info.feed(x) {
                    self.heard(Side::Call, info);
                }
                if let Some(info) = self.answer_info.feed(x) {
                    self.heard(Side::Answer, info);
                }
            }
            V34Step::Answerer(third) => {
                third.feed(x, &mut self.said, &mut self.notes);
                if third.over(self.fs) {
                    // The call modem's turn: its S comes in as the answer
                    // modem's J is ending.
                    self.step = match self.call_band {
                        Some(band) => V34Step::Caller(Box::new(Third::new(band, Mode::Call, "call", self.fs))),
                        None => V34Step::Both,
                    };
                }
            }
            V34Step::Caller(third) => {
                third.feed(x, &mut self.said, &mut self.notes);
                if third.over(self.fs) {
                    self.notes.push(BOTH.to_owned());
                    self.step = V34Step::Both;
                }
            }
            V34Step::Both => {}
        }
    }

    /// An INFO sequence, whose CRC has checked: say what it says, and on
    /// INFO1a -- the last of phase 2 -- start listening for phase 3 on the
    /// bands it settles.
    fn heard(&mut self, side: Side, info: Info) {
        let who = if side == Side::Call { "call" } else { "answer" };
        let bits = |n: u8| u32::from(n) * 2400;
        match info {
            Info::Info0(i) => {
                let mut rates = vec!["2400"];
                for (has, name) in [
                    (i.rate_2743, "2743"),
                    (i.rate_2800, "2800"),
                    (i.low_carrier_3000 || i.high_carrier_3000, "3000"),
                    (i.low_carrier_3200 || i.high_carrier_3200, "3200"),
                    (i.rate_3429, "3429"),
                ] {
                    if has {
                        rates.push(name);
                    }
                }
                let text = format!(
                    "symbol rates {}{}",
                    rates.join(", "),
                    if i.constellation_1664 { " · 1664 points" } else { "" }
                );
                self.say(if side == Side::Call { "caller INFO0" } else { "answerer INFO0" }, who, "INFO0", text);
            }
            Info::Info1c(i) => {
                let text = SymbolRate::ALL
                    .iter()
                    .zip(i.probed)
                    .filter(|(_, p)| p.max_rate > 0)
                    .map(|(rate, p)| format!("{} Bd: {}", rate.nominal(), bits(p.max_rate)))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.info1c = Some(i);
                self.say("caller INFO1c", who, "INFO1c: what the probing found each symbol rate would carry", text);
            }
            Info::Info1a(i) => {
                let text = format!(
                    "{} Bd to the caller, {} Bd back at up to {} bit/s",
                    i.answer_to_call.nominal(),
                    i.call_to_answer.nominal(),
                    bits(i.probed.max_rate)
                );
                self.say("answerer INFO1a", who, "INFO1a", text);
                // The answer modem's carrier is the one the call modem asked
                // for at that symbol rate, in INFO1c.
                let high = self
                    .info1c
                    .is_some_and(|c| c.probed[i.answer_to_call.index() as usize].high_carrier);
                self.call_band = Some(Band::new(i.call_to_answer, i.probed.high_carrier));
                self.step = V34Step::Answerer(Box::new(Third::new(
                    Band::new(i.answer_to_call, high),
                    Mode::Answer,
                    "answer",
                    self.fs,
                )));
            }
            // Half duplex, V.90 and V.92 have their own start-ups, which this does
            // not follow.
            Info::InfoH(_)
            | Info::Info0d(_)
            | Info::Info1aPcm(_)
            | Info::Info1aPcmUp(_)
            | Info::Info1aV34Up(_)
            | Info::Mh(_) => {
                self.notes.push(format!("the {who} modem sent an INFO sequence of another start-up"));
            }
        }
    }

    fn say(&mut self, key: &'static str, who: &str, what: &str, text: String) {
        self.notes.push(format!("the {who} modem's {what}: {text}"));
        self.said.push((key, text));
    }

    fn third(&self) -> Option<&Third> {
        match &self.step {
            V34Step::Answerer(third) | V34Step::Caller(third) => Some(third),
            _ => None,
        }
    }

    pub fn point(&self) -> Option<(f32, f32)> {
        self.third().filter(|t| t.drawing()).and_then(|t| t.point)
    }

    pub fn snr_db(&self) -> Option<f32> {
        self.third().filter(|t| t.drawing()).map(|t| t.rx.snr_db() as f32)
    }

    pub fn training(&self) -> bool {
        self.third().is_some_and(Third::drawing)
    }

    pub fn phase(&self) -> &'static str {
        match &self.step {
            V34Step::Second => "V.8 and phase 2: listening for the INFO sequences",
            V34Step::Answerer(t) if t.drawing() => "phase 3: the answer modem's TRN, alone on the line",
            V34Step::Answerer(_) => "phase 3: waiting for the answer modem's S",
            V34Step::Caller(t) if t.drawing() => "phase 3: the call modem's TRN, alone on the line",
            V34Step::Caller(_) => "phase 3: waiting for the call modem's S",
            V34Step::Both => BOTH,
        }
    }

    pub fn said(&self) -> &[(&'static str, String)] {
        &self.said
    }

    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector(name: &str) -> (Vec<f32>, f64) {
        let path = format!("{}/../../tests/vectors/{name}", env!("CARGO_MANIFEST_DIR"));
        let wav = line::wav::read(&path).expect("could not read the vector");
        (wav.mono(), f64::from(wav.sample_rate))
    }

    /// The recorded V.32 call: both conditioning signals train a receiver and
    /// both rate signals are read, with no cue but the signal itself.
    #[test]
    fn a_v32_call_on_one_tap_is_followed_through_both_trainings() {
        let (samples, fs) = vector("v32bis-14400.wav");
        let mut tap = V32Tap::new(fs);
        let mut drawn = [0usize; 2];
        let mut notes = Vec::new();
        for (n, &x) in samples.iter().enumerate() {
            tap.feed(f64::from(x));
            if tap.point().is_some() {
                drawn[usize::from(tap.step == V32Step::Caller)] += 1;
            }
            for note in tap.take_notes() {
                notes.push(format!("{:6.3} s {note}", n as f64 / fs));
            }
        }
        println!("{}", notes.join("\n"));
        let keys: Vec<&str> = tap.said().iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, ["answerer R1", "caller R2"], "{:?}", tap.said());
        assert_eq!(tap.phase(), BOTH);
        // Each drew symbols for a good second of its TRN.
        assert!(drawn[0] > fs as usize, "answerer drawn for {} samples", drawn[0]);
        assert!(drawn[1] > (fs * 0.5) as usize, "caller drawn for {} samples", drawn[1]);
    }

    /// The recorded V.34 call: three INFO sequences, and each modem's phase 3
    /// trained on and read through to its J.
    #[test]
    fn a_v34_call_on_one_tap_is_followed_into_phase_3() {
        let (samples, fs) = vector("v34-33600.wav");
        let mut tap = V34Tap::new(fs);
        let mut drawn = 0usize;
        let mut notes = Vec::new();
        for (n, &x) in samples.iter().enumerate() {
            tap.feed(f64::from(x));
            drawn += usize::from(tap.point().is_some());
            for note in tap.take_notes() {
                notes.push(format!("{:6.3} s {note}", n as f64 / fs));
            }
        }
        println!("{}", notes.join("\n"));
        let keys: Vec<&str> = tap.said().iter().map(|(k, _)| *k).collect();
        assert!(
            keys == ["answerer INFO0", "caller INFO1c", "answerer INFO1a", "answerer J", "caller J"],
            "{:?}",
            tap.said()
        );
        assert!(drawn > fs as usize, "drawn for {drawn} samples");
        assert_eq!(tap.phase(), BOTH);
    }
}
