//! A fax call on the line: the tones, V.21, the page carriers, and T.30 above
//! them.
//!
//! The join between the two halves. [`fax::call`] knows the procedure and
//! nothing about signals; [`datapump::v21`], [`datapump::v27ter`] and
//! [`datapump::v29`] know the signals and nothing about the procedure. This
//! puts one on top of the other and gives the result a sample at a time, which
//! is the only thing a line understands.
//!
//! The whole of the join is one question asked once a sample: what should be
//! on the line just now. A fax call answers it with a different thing eight or
//! ten times before a page has moved -- a tone, then 300 bit/s, then silence,
//! then 9600, then silence, then 300 again -- and every one of those changes
//! is a carrier going up or down at both ends.

use datapump::{v21, v27ter, v29};
use fax::call::{Call, Line, Phase, Role, Speed};
use fax::page::{Page, Resolution};
use fax::t30::Modulation;

/// Which page carrier a speed calls for, and at which of its rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    V27ter(v27ter::Rate),
    V29(v29::Rate),
}

impl Carrier {
    /// `None` for anything this end has no pump for, which only ever happens
    /// when the far end's DCS names one: the ladder this end climbs is built
    /// from what it has.
    fn of(speed: Speed) -> Option<Self> {
        Some(match (speed.modulation, speed.bits_per_second) {
            (Modulation::V27ter, 4800) => Self::V27ter(v27ter::Rate::R4800),
            (Modulation::V27ter, 2400) => Self::V27ter(v27ter::Rate::R2400),
            (Modulation::V29, 9600) => Self::V29(v29::Rate::R9600),
            (Modulation::V29, 7200) => Self::V29(v29::Rate::R7200),
            (Modulation::V29, 4800) => Self::V29(v29::Rate::R4800),
            _ => return None,
        })
    }
}

/// A fax call, from either end.
#[derive(Debug)]
pub struct FaxCall {
    call: Call,
    control_tx: v21::Sender,
    control_rx: v21::Receiver,
    v27ter_tx: v27ter::Transmitter,
    v27ter_rx: v27ter::Receiver,
    v29_tx: v29::Transmitter,
    v29_rx: v29::Receiver,
    cng: v21::Tone,
    ced: v21::Tone,
    /// What the line was doing on the last sample, so a change can be seen.
    line: Line,
}

impl FaxCall {
    /// The end that dialled.
    pub fn originate(fs: f64, identification: &str, page: Option<Page>) -> Self {
        Self::with(Call::originate(fs, identification, page), fs)
    }

    /// The end that answered.
    pub fn answer(fs: f64, identification: &str) -> Self {
        Self::with(Call::answer(fs, identification), fs)
    }

    fn with(call: Call, fs: f64) -> Self {
        Self {
            call,
            control_tx: v21::Sender::new(fs),
            control_rx: v21::Receiver::new(fs),
            v27ter_tx: v27ter::Transmitter::new(fs),
            v27ter_rx: v27ter::Receiver::new(fs),
            v29_tx: v29::Transmitter::new(fs),
            v29_rx: v29::Receiver::new(fs),
            cng: v21::Tone::new(v21::CNG, fs),
            ced: v21::Tone::new(v21::CED, fs),
            line: Line::Quiet,
        }
    }

    /// Use only these modulations: what goes in this end's DIS, and what it
    /// will choose from when it sends.
    #[must_use]
    pub fn offering(mut self, modulations: &[Modulation]) -> Self {
        self.call.set_offer(modulations);
        self
    }

    /// Put V.27 ter's protection against talker echo in front of every burst:
    /// a fifth of a second of plain carrier, then twenty milliseconds of
    /// nothing, then the training.
    #[must_use]
    pub fn with_echo_protection(mut self, on: bool) -> Self {
        self.v27ter_tx.set_echo_protection(on);
        self
    }

    /// Offer error correction mode, or not.
    #[must_use]
    pub fn with_error_correction(mut self, on: bool) -> Self {
        self.call.set_error_correction(on);
        self
    }

    /// Whether this call is in error correction mode.
    pub fn error_correction(&self) -> bool {
        self.call.error_correction()
    }

    pub fn role(&self) -> Role {
        self.call.role()
    }

    pub fn phase(&self) -> Phase {
        self.call.phase()
    }

    pub fn seconds(&self) -> f64 {
        self.call.seconds()
    }

    pub fn identity(&self) -> &str {
        &self.call.identity
    }

    pub fn capabilities(&self) -> Option<&fax::t30::Capabilities> {
        self.call.capabilities.as_ref()
    }

    /// The capability field as it arrived.
    pub fn capability_field(&self) -> Option<&[u8]> {
        self.call.capability_field.as_deref()
    }

    /// The rate the page is being carried at.
    pub fn rate(&self) -> u32 {
        self.call.rate()
    }

    /// The modulation and rate the page is being carried at.
    pub fn speed(&self) -> Speed {
        self.call.speed()
    }

    /// How far through the page the call has got.
    pub fn progress(&self) -> Option<f64> {
        self.call.progress()
    }

    /// Lines of a page that have arrived.
    pub fn lines_received(&self) -> usize {
        self.call.lines_received()
    }

    /// The lines of the page arriving, as far as it has been decoded.
    pub fn lines(&self) -> &[Vec<bool>] {
        self.call.lines()
    }

    /// The resolution the page is arriving at, as the DCS said.
    pub fn resolution(&self) -> Resolution {
        self.call.resolution()
    }

    /// The page that arrived, once one has.
    pub fn received(&self) -> Option<&Page> {
        self.call.received.as_ref()
    }

    /// The page that arrived, handed over and forgotten.
    ///
    /// A page is a couple of megabytes of booleans, so it is moved rather
    /// than copied and moved exactly once. Whoever takes it owns it.
    pub fn take_received(&mut self) -> Option<Page> {
        self.call.received.take()
    }

    /// Why the call went badly, if it did.
    pub fn trouble(&self) -> Option<&str> {
        self.call.trouble.as_deref()
    }

    pub fn take_heard(&mut self) -> Vec<fax::frames::Message> {
        self.call.take_heard()
    }

    /// Whether anything of the far end's is on the line.
    pub fn carrier(&self) -> bool {
        self.control_rx.carrier() || self.v27ter_rx.carrier() || self.v29_rx.carrier()
    }

    /// The page carrier the line is on just now, if it is on one.
    ///
    /// A fax call has three receivers and only ever one of them is the one
    /// that matters. Which, decides what a scope should be drawing: the page
    /// carriers have constellations and the control channel has an eye, and
    /// those are not the same picture at all.
    fn page_carrier(&self) -> Option<Carrier> {
        match self.line {
            Line::Fast(speed) | Line::FastListen(speed) => Carrier::of(speed),
            _ => None,
        }
    }

    /// The constellation on the line just now, while a page carrier is.
    ///
    /// Whichever end of it this is. Sending, it is the points going out --
    /// there is nothing arriving on a half-duplex line to draw instead, and a
    /// scope that froze on the last point it received would show a training
    /// sequence as a single dot. Listening, it is the points the receiver
    /// decided on, but only while it hears a carrier: the silence either side
    /// of a burst comes out of an equaliser as a smear at the centre that is
    /// not a picture of anything.
    pub fn constellation_point(&self) -> Option<(f64, f64)> {
        match self.line {
            Line::Fast(speed) => match Carrier::of(speed)? {
                Carrier::V27ter(_) => self.v27ter_tx.last_point(),
                Carrier::V29(_) => self.v29_tx.last_point(),
            },
            Line::FastListen(speed) => match Carrier::of(speed)? {
                Carrier::V27ter(_) => self
                    .v27ter_rx
                    .carrier()
                    .then(|| self.v27ter_rx.constellation_point()),
                Carrier::V29(_) => self
                    .v29_rx
                    .carrier()
                    .then(|| self.v29_rx.constellation_point()),
            },
            _ => None,
        }
    }

    /// How far out that constellation reaches.
    ///
    /// One for V.27 ter, whose points are on the unit circle. V.29's outer
    /// ring on the axes is a third beyond it, and a scope drawn to the unit
    /// circle would put four of its sixteen points off the edge.
    pub fn constellation_peak(&self) -> f64 {
        match self.page_carrier() {
            Some(Carrier::V29(_)) => self.v29_rx.constellation_peak(),
            _ => 1.0,
        }
    }

    /// The control channel's discriminator, while that is the one in use.
    pub fn discriminator(&self) -> Option<f64> {
        self.on_the_control_channel()
            .then(|| self.control_rx.discriminator())
    }

    /// One reading per recovered bit of the control channel.
    pub fn take_symbol(&mut self) -> Option<f64> {
        if !self.on_the_control_channel() {
            return None;
        }
        self.control_rx.take_symbol()
    }

    fn on_the_control_channel(&self) -> bool {
        !matches!(self.line, Line::Fast(_) | Line::FastListen(_))
    }

    /// Mean distance from the decisions being made, where there are points to
    /// decide between.
    pub fn residual_error(&self) -> Option<f64> {
        Some(match self.page_carrier()? {
            Carrier::V27ter(_) => self.v27ter_rx.residual_error(),
            Carrier::V29(_) => self.v29_rx.residual_error(),
        })
    }

    /// That distance as a fraction of the gap between neighbouring points,
    /// where half is the decision boundary.
    pub fn reception(&self) -> Option<f64> {
        Some(match self.page_carrier()? {
            Carrier::V27ter(_) => {
                self.v27ter_rx.residual_error() / self.v27ter_rx.point_spacing()
            }
            Carrier::V29(_) => self.v29_rx.residual_error() / self.v29_rx.point_spacing(),
        })
    }

    /// How many points the scope should expect.
    pub fn states(&self) -> usize {
        match self.page_carrier() {
            None => 2,
            Some(Carrier::V27ter(rate)) => usize::from(rate.phases()),
            Some(Carrier::V29(rate)) => rate.constellation().len(),
        }
    }

    /// Short name for the signal shape, as a faceplate would print it.
    ///
    /// V.29 is amplitude and phase rather than a square grid, so its names
    /// say so: two radii on each of the eight phases is not what "16QAM" makes
    /// anybody picture.
    pub fn shape(&self) -> &'static str {
        match self.page_carrier() {
            None => "2FSK",
            Some(Carrier::V27ter(v27ter::Rate::R4800)) => "8PSK",
            Some(Carrier::V27ter(v27ter::Rate::R2400)) => "4PSK",
            Some(Carrier::V29(v29::Rate::R9600)) => "16APM",
            Some(Carrier::V29(v29::Rate::R7200)) => "8APM",
            Some(Carrier::V29(v29::Rate::R4800)) => "4PSK",
        }
    }

    /// The modulation carrying the line just now.
    pub fn standard(&self) -> &'static str {
        match self.page_carrier() {
            None => "V.21",
            Some(Carrier::V27ter(_)) => "V.27ter",
            Some(Carrier::V29(_)) => "V.29",
        }
    }

    /// One sample in, one sample out.
    pub fn step(&mut self, input: f64) -> f64 {
        let want = self.call.line();
        self.follow(want);
        self.listen(want, input);
        let (out, idle) = self.talk(want);
        self.call.tick(idle);
        out
    }

    /// Put up or take down whatever changed.
    fn follow(&mut self, want: Line) {
        if want == self.line {
            return;
        }
        match self.line {
            Line::Control => self.control_tx.set_transmitting(false),
            // Nothing should be left running by the time the procedure moves
            // on, since it waits for the line to go idle first. This is only
            // in case something ends a call in the middle of a burst.
            Line::Fast(_) => {
                self.v27ter_tx.abort();
                self.v29_tx.abort();
            }
            _ => {}
        }
        match want {
            Line::Control => self.control_tx.set_transmitting(true),
            Line::Fast(speed) => match Carrier::of(speed) {
                // Always V.27 ter's long turn-on sequence. T.30 leaves the
                // choice to the sender, and a fax turns the line around between
                // every message, so nothing is remembered from the last burst
                // that a short one could refresh. V.29 has only the one.
                Some(Carrier::V27ter(rate)) => {
                    self.v27ter_tx.start(rate, v27ter::Training::Long);
                }
                Some(Carrier::V29(rate)) => self.v29_tx.start(rate),
                None => {}
            },
            Line::FastListen(speed) => match Carrier::of(speed) {
                Some(Carrier::V27ter(rate)) => {
                    self.v27ter_rx.set_rate(rate);
                    self.v27ter_rx.restart();
                }
                Some(Carrier::V29(rate)) => {
                    self.v29_rx.set_rate(rate);
                    self.v29_rx.restart();
                }
                None => {}
            },
            _ => {}
        }
        self.line = want;
    }

    /// Feed whichever receiver belongs to what the line is doing.
    ///
    /// Only one of them, and only while this end is not talking. On a
    /// two-wire line a receiver left running hears its own transmission, and
    /// a fax is half duplex, so anything it hears while sending is its own
    /// echo. The frames in that echo are the frames it just sent, addressed
    /// the same way, and would be read as the far end agreeing with itself.
    fn listen(&mut self, want: Line, input: f64) {
        match want {
            Line::Quiet | Line::Listen | Line::CallingTone => {
                if let Some(bit) = self.control_rx.feed(input) {
                    self.call.control_bit(bit);
                }
                self.call.set_control_carrier(self.control_rx.carrier());
            }
            Line::FastListen(speed) => {
                // The control channel too. Waiting for a page carrier is when
                // a sender whose last command went unanswered sends it again,
                // and under error correction mode that is the ordinary way to
                // recover a lost confirmation. What a V.21 receiver makes of a
                // page carrier is noise, and noise does not pass a frame check.
                if let Some(bit) = self.control_rx.feed(input) {
                    self.call.control_bit(bit);
                }
                let (bits, carrier) = match Carrier::of(speed) {
                    Some(Carrier::V27ter(_)) => {
                        self.v27ter_rx.feed(input);
                        (self.v27ter_rx.take_bits(), self.v27ter_rx.carrier())
                    }
                    Some(Carrier::V29(_)) => {
                        self.v29_rx.feed(input);
                        (self.v29_rx.take_bits(), self.v29_rx.carrier())
                    }
                    // A speed this end has no receiver for hears nothing, and
                    // the training check that never arrives is refused, which
                    // sends the far end down its own ladder.
                    None => (Vec::new(), false),
                };
                if !bits.is_empty() {
                    self.call.fast_bits(&bits);
                }
                self.call.set_fast_carrier(carrier);
            }
            Line::Control | Line::Fast(_) | Line::CalledTone => {}
        }
    }

    /// Produce the sample, and say whether the line has gone quiet.
    fn talk(&mut self, want: Line) -> (f64, bool) {
        match want {
            Line::Control => {
                while self.control_tx.pending_bits() < 16 {
                    match self.call.next_control_bit() {
                        Some(bit) => self.control_tx.push_bits(&[bit]),
                        None => break,
                    }
                }
                let idle = self.control_tx.pending_bits() == 0;
                (self.control_tx.next_sample(), idle)
            }
            Line::Fast(speed) => match Carrier::of(speed) {
                Some(Carrier::V27ter(_)) => {
                    while self.v27ter_tx.pending_bits() < 32 {
                        match self.call.next_fast_bit() {
                            Some(bit) => self.v27ter_tx.push_bits(&[bit]),
                            None => break,
                        }
                    }
                    // Nothing left to hand over and nothing left in the
                    // modulator: the burst is over, so take the carrier down
                    // with a turn-off rather than cutting it.
                    if self.v27ter_tx.trained() && self.v27ter_tx.pending_bits() == 0 {
                        self.v27ter_tx.stop();
                    }
                    let idle = !self.v27ter_tx.is_transmitting();
                    (self.v27ter_tx.next_sample(), idle)
                }
                Some(Carrier::V29(_)) => {
                    while self.v29_tx.pending_bits() < 32 {
                        match self.call.next_fast_bit() {
                            Some(bit) => self.v29_tx.push_bits(&[bit]),
                            None => break,
                        }
                    }
                    if self.v29_tx.trained() && self.v29_tx.pending_bits() == 0 {
                        self.v29_tx.stop();
                    }
                    let idle = !self.v29_tx.is_transmitting();
                    (self.v29_tx.next_sample(), idle)
                }
                None => (0.0, true),
            },
            Line::CallingTone => {
                let on = self.call.calling_tone_on();
                // The tone keeps running while it is silent, so its phase is
                // continuous across the gaps rather than clicking at every
                // burst.
                let sample = self.cng.next_sample();
                (if on { sample } else { 0.0 }, true)
            }
            Line::CalledTone => (self.ced.next_sample(), true),
            Line::Quiet | Line::Listen | Line::FastListen(_) => (0.0, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fax::frames::{Message, Reader};
    use fax::page::Resolution;
    use fax::t30::Frame;

    const FS: f64 = 16_000.0;

    /// A page with something recognisable on it.
    fn a_page(lines: usize) -> Page {
        let width = fax::page::WIDTH;
        Page {
            lines: (0..lines)
                .map(|y| {
                    (0..width)
                        .map(|x| (x / 40 + y / 8).is_multiple_of(2) && x % 40 < 30)
                        .collect()
                })
                .collect(),
            resolution: Resolution::Standard,
        }
    }

    /// Run two of these against each other down one clean wire.
    fn between(caller: &mut FaxCall, answerer: &mut FaxCall, seconds: f64) {
        through(caller, answerer, seconds, &mut |s| s);
    }

    /// The same, with something done to the line in both directions.
    ///
    /// `FAX_TRACE=1` prints every change of phase at both ends. A fax call
    /// goes wrong by one end waiting for something the other has stopped
    /// sending, and that is invisible in an assertion at the end of it.
    fn through(
        caller: &mut FaxCall,
        answerer: &mut FaxCall,
        seconds: f64,
        line: &mut dyn FnMut(f64) -> f64,
    ) {
        let trace = std::env::var("FAX_TRACE").is_ok();
        let (mut was_a, mut was_b) = (caller.phase(), answerer.phase());
        let mut from_caller = 0.0;
        let mut from_answerer = 0.0;
        for i in 0..(seconds * FS) as usize {
            let a = caller.step(from_answerer);
            let b = answerer.step(from_caller);
            from_caller = line(a);
            from_answerer = line(b);
            if trace && (caller.phase() != was_a || answerer.phase() != was_b) {
                eprintln!(
                    "{:6.2}s caller {:<32} answerer {}",
                    i as f64 / FS,
                    caller.phase().name(),
                    answerer.phase().name()
                );
                was_a = caller.phase();
                was_b = answerer.phase();
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
    }

    #[test]
    fn the_calling_tone_goes_on_the_line() {
        let mut call = FaxCall::originate(FS, "61400000000", None);
        let mut loudest = 0.0f64;
        for _ in 0..(FS * 0.3) as usize {
            loudest = loudest.max(call.step(0.0).abs());
        }
        assert!(loudest > 0.1, "nothing went out: {loudest}");
    }

    #[test]
    fn the_answering_tone_goes_on_the_line() {
        let mut call = FaxCall::answer(FS, "61399990000");
        let mut loudest = 0.0f64;
        for _ in 0..(FS * 0.3) as usize {
            loudest = loudest.max(call.step(0.0).abs());
        }
        assert!(loudest > 0.1, "nothing went out: {loudest}");
    }

    #[test]
    fn a_machine_answering_is_heard_and_answered() {
        // The far end is a recording of a real one: its identification and
        // its capabilities, sent on V.21 as it would send them.
        let mut far_tx = fax::frames::Sender::new();
        far_tx.send(&[
            Message::new(Frame::Csi, false)
                .and_more()
                .with_fif(b"       909 863  0031"),
            Message::new(Frame::Dis, false).with_fif(&[0x00, 0x6e, 0xf8, 0x00]),
        ]);
        let mut far = v21::Sender::new(FS);
        far.set_transmitting(true);

        let mut call = FaxCall::originate(FS, "61400000000", None);
        let mut ours = v21::Receiver::new(FS);
        let mut reader = Reader::new();
        let mut said: Vec<Message> = Vec::new();

        for _ in 0..(FS * 20.0) as usize {
            while far.pending_bits() < 16 {
                match far_tx.next_bit() {
                    Some(b) => far.push_bits(&[b]),
                    None => break,
                }
            }
            let from_far = far.next_sample();
            let from_us = call.step(from_far);
            if let Some(bit) = ours.feed(from_us)
                && let Some(m) = reader.feed(bit)
            {
                said.push(m);
            }
            if call.phase().is_over() {
                break;
            }
        }

        assert_eq!(call.identity(), "1300  368 909");
        let caps = call.capabilities().expect("it said what it can do");
        assert_eq!(caps.modulations.len(), 3, "V.27ter, V.29 and V.17");

        let names: Vec<Frame> = said.iter().map(|m| m.frame).collect();
        assert!(
            names.contains(&Frame::Tsi),
            "this end never identified itself: {names:?}"
        );
        assert!(
            names.contains(&Frame::Dcs),
            "this end never said how it would send: {names:?}"
        );
    }

    #[test]
    fn one_modem_faxes_a_page_to_another() {
        let page = a_page(8);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        between(&mut caller, &mut answerer, 40.0);

        assert_eq!(
            caller.phase(),
            Phase::Done,
            "the caller ended at {} ({:?})",
            caller.phase().name(),
            caller.trouble()
        );
        assert_eq!(
            answerer.phase(),
            Phase::Done,
            "the answerer ended at {} ({:?})",
            answerer.phase().name(),
            answerer.trouble()
        );
        let got = answerer.received().expect("no page arrived");
        assert_eq!(got.lines.len(), page.lines.len(), "wrong number of lines");
        assert_eq!(got.lines, page.lines, "the page came out different");
    }


    /// A page over a line with noise on it.
    ///
    /// Not a real channel -- there is no filtering and no echo -- but enough
    /// to prove that the page is not getting through because both ends are
    /// working from arithmetic that happens to match. A single bit error in
    /// the training check used to throw the rate away, and a single one in
    /// the page has to cost one line rather than the page.
    #[test]
    fn a_page_gets_through_a_line_with_noise_on_it() {
        let page = a_page(6);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut noise = move || {
            // Xorshift, so the run is the same every time it is looked at.
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64 * 0.02 - 0.01
        };
        through(&mut caller, &mut answerer, 40.0, &mut |s| s + noise());
        let got = answerer.received().expect("no page arrived");
        assert_eq!(got.lines, page.lines, "the page came out different");
    }

    /// A page over a line that is simply quiet.
    ///
    /// Thirty decibels down is about what a real one delivered, once the
    /// drive setting and the path between the two machines had had it. The
    /// carrier detector had a threshold picked from a loopback, where the far
    /// end is exactly as loud as this end wrote it, so it never saw the
    /// carrier at all -- and everything downstream of a carrier detector is
    /// held still until it says there is something there.
    #[test]
    fn a_page_gets_through_a_line_thirty_decibels_down() {
        let page = a_page(6);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        through(&mut caller, &mut answerer, 40.0, &mut |s| s * 0.0316);
        let got = answerer.received().expect("no page arrived");
        assert_eq!(got.lines, page.lines, "the page came out different");
    }


    /// A fax call has something to put on the scope the whole way through.
    ///
    /// Two different pictures, because there are two carriers. The 300 bit/s
    /// channel is frequency shift keying and what it has is an eye; the page
    /// carrier -- V.29 at 9600, between two of these -- is sixteen points on
    /// two radii, and what it has is a constellation. A panel showing neither for the whole of a call is a
    /// panel that has nothing to say about the one modulation in the call
    /// that can actually go wrong.
    #[test]
    fn both_carriers_of_a_fax_call_reach_the_scope() {
        let page = a_page(6);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");

        let mut shapes: Vec<&str> = Vec::new();
        let mut eye = 0usize;
        let mut points: Vec<(f64, f64)> = Vec::new();
        let mut worst_reception = 0.0f64;

        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        for _ in 0..(FS * 40.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;

            let shape = answerer.shape();
            if shapes.last() != Some(&shape) {
                shapes.push(shape);
            }
            if answerer.take_symbol().is_some() {
                eye += 1;
            }
            if let Some(p) = answerer.constellation_point() {
                points.push(p);
            }
            // Only while the page is actually moving: before the carrier
            // arrives the equaliser has nothing to be right or wrong about.
            if answerer.phase() == Phase::Receiving
                && answerer.lines_received() > 2
                && let Some(r) = answerer.reception()
            {
                worst_reception = worst_reception.max(r);
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }

        assert!(answerer.received().is_some(), "the page did not arrive");
        assert!(
            shapes.contains(&"2FSK") && shapes.contains(&"16APM"),
            "the scope was never told what it was drawing: {shapes:?}"
        );
        assert!(eye > 500, "only {eye} readings for the eye");
        assert!(points.len() > 1000, "only {} points", points.len());

        // V.29's points run from the inner diagonals, at the square root of
        // 2 over that of 13.5, to the outer axes at 5 over it: 0.38 to 1.36
        // with the mean power made one. Anything well outside is not a point.
        let strays = points
            .iter()
            .filter(|(x, y)| {
                let r = (x * x + y * y).sqrt();
                !(0.2..1.6).contains(&r)
            })
            .count();
        assert!(
            strays * 20 < points.len(),
            "{strays} of {} points were nowhere near the circle",
            points.len()
        );
        assert!(
            worst_reception < 0.25,
            "the receiver was missing by {worst_reception:.2} of a gap, and \
             half is the decision boundary"
        );
    }

    /// What a real fax machine sends in front of its training.
    ///
    /// A public fax service sending to this modem put V.27 ter's protection
    /// against talker echo in front of every training check: a fifth of a
    /// second of plain carrier, twenty milliseconds of silence, then the
    /// training. The carrier going away for those twenty milliseconds was
    /// taken for the end of the burst, so the answering end judged a training
    /// check made of nothing but that plain carrier, refused it, and did the
    /// same again at 2400 until the far end gave up. Its receiver had read the
    /// training check perfectly -- 7195 zeros out of 7200, both times.
    #[test]
    fn a_silence_inside_the_training_is_not_the_end_of_the_burst() {
        let page = a_page(6);
        let mut caller = FaxCall::originate(FS, "1300368909", Some(page.clone()))
            .offering(&[Modulation::V27ter])
            .with_echo_protection(true);
        let mut answerer = FaxCall::answer(FS, "61388880000");
        between(&mut caller, &mut answerer, 40.0);
        let got = answerer
            .received()
            .unwrap_or_else(|| panic!("no page arrived ({:?})", answerer.trouble()));
        assert_eq!(got.lines, page.lines, "the page came out different");
        assert_eq!(caller.rate(), 4800, "the far end had to drop a rate to get through");
    }


    /// The end that is sending draws what it sends.
    ///
    /// A fax is half duplex, so while the training goes out there is nothing
    /// arriving to draw. The panel showed the control channel's eye instead,
    /// which reads as frequency shift keying in the middle of a burst that is
    /// nothing of the kind.
    #[test]
    fn the_sending_end_draws_the_constellation_it_is_sending() {
        let mut caller = FaxCall::originate(FS, "61399990000", Some(a_page(4)));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        let mut during_training: Vec<&str> = Vec::new();
        let mut radii: Vec<f64> = Vec::new();
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        for _ in 0..(FS * 40.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;
            if matches!(caller.phase(), Phase::Training | Phase::Sending) {
                let shape = caller.shape();
                if during_training.last() != Some(&shape) {
                    during_training.push(shape);
                }
                if let Some((x, y)) = caller.constellation_point() {
                    radii.push((x * x + y * y).sqrt());
                }
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        assert!(
            during_training.contains(&"16APM"),
            "the sending end never said it was sending V.29: {during_training:?}"
        );
        assert!(radii.len() > 10_000, "only {} points while sending", radii.len());
        // Every one of them exactly a point of Figure 1, since these are the
        // points that were sent rather than a receiver's guess at them.
        let rms = 13.5f64.sqrt();
        let figure = [2f64.sqrt(), 3.0, 18f64.sqrt(), 5.0].map(|r| r / rms);
        let off = radii
            .iter()
            .filter(|r| figure.iter().all(|f| (*r - f).abs() > 1e-9))
            .count();
        assert_eq!(off, 0, "{off} points sent were not points of the constellation");
    }


    /// A fine page goes both ways as a fine page, coded two-dimensionally.
    ///
    /// Two of these offer each other both of T.4's codings and both
    /// resolutions, so this is the page as it should arrive: every line, at the
    /// resolution it was drawn at, in the coding that makes it smallest.
    #[test]
    fn a_fine_page_arrives_fine_and_coded_two_dimensionally() {
        let mut page = a_page(12);
        page.resolution = Resolution::Fine;
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        between(&mut caller, &mut answerer, 40.0);
        let got = answerer.received().expect("no page arrived");
        assert_eq!(got.resolution, Resolution::Fine, "it arrived as standard");
        assert_eq!(got.lines, page.lines, "the page came out different");
    }

    /// Run a call with a burst of noise dropped onto the page the first time
    /// it goes out, and hand back every frame the answering end heard.
    fn with_a_burst_of_noise(caller: &mut FaxCall, answerer: &mut FaxCall) -> Vec<Frame> {
        let mut heard = Vec::new();
        let mut hit = false;
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        let mut seed = 0x1234_5678u32;
        let mut started: Option<f64> = None;
        for _ in 0..(FS * 60.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            if started.is_none()
                && caller.phase() == Phase::Sending
                && caller.progress().is_some_and(|p| p > 0.3)
            {
                started = Some(caller.seconds());
            }
            let during = !hit && started.is_some_and(|t| caller.seconds() - t < 0.1);
            to_answerer = if during {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                // Loud enough to spoil every symbol it lands on.
                a + (f64::from(seed) / f64::from(u32::MAX) - 0.5) * 2.0
            } else {
                a
            };
            if caller.phase() == Phase::EndingPage && caller.progress().is_some() {
                hit = true;
            }
            to_caller = b;
            heard.extend(answerer.take_heard().into_iter().map(|m| m.frame));
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        heard
    }

    fn a_long_page(rows: usize) -> Page {
        let width = fax::page::WIDTH;
        Page {
            lines: (0..rows)
                .map(|y| {
                    (0..width)
                        .map(|x| (x / 7 + y / 3).is_multiple_of(3) && (x * 13 + y * 7) % 11 < 6)
                        .collect()
                })
                .collect(),
            resolution: Resolution::Standard,
        }
    }

    #[test]
    fn two_of_these_use_error_correction_mode() {
        let page = a_page(8);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        let mut sent = Vec::new();
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        for _ in 0..(FS * 40.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;
            sent.extend(answerer.take_heard().into_iter().map(|m| m.frame));
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        assert!(caller.error_correction(), "the caller did not choose it");
        assert!(answerer.error_correction(), "the answerer was not told");
        assert!(sent.contains(&Frame::Pps), "no partial page signal: {sent:?}");
        assert!(!sent.contains(&Frame::Eop), "a bare EOP under error correction");
        assert_eq!(answerer.received().expect("no page").lines, page.lines);
    }

    /// Run a call and watch the answering end's lines, handing back every
    /// count of them seen while the page was still arriving.
    fn watch_it_arrive(caller: &mut FaxCall, answerer: &mut FaxCall) -> Vec<usize> {
        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        let mut seen = Vec::new();
        for _ in 0..(FS * 60.0) as usize {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;
            if answerer.phase() == Phase::Receiving && answerer.received().is_none() {
                let lines = answerer.lines().len();
                assert_eq!(lines, answerer.lines_received());
                if seen.last() != Some(&lines) {
                    seen.push(lines);
                }
            }
            if caller.phase().is_over() && answerer.phase().is_over() {
                break;
            }
        }
        seen
    }

    #[test]
    fn a_page_can_be_watched_arriving_with_error_correction_or_without() {
        // Without it the lines come off the decoder as the bits do. With it
        // they used to come all at once at the end, because the page was only
        // decoded once every block of it was in -- and a page that appears
        // whole a minute after it started is not one anybody can watch
        // arriving.
        for error_correction in [true, false] {
            let page = a_page(400);
            let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
            let mut answerer =
                FaxCall::answer(FS, "61388880000").with_error_correction(error_correction);
            let seen = watch_it_arrive(&mut caller, &mut answerer);
            assert_eq!(caller.error_correction(), error_correction);
            let got = answerer.received().expect("no page");
            assert_eq!(got.lines, page.lines, "error correction {error_correction}");
            assert_eq!(answerer.lines(), &page.lines[..], "the lines are not the page");
            // A page drawn as it comes is one seen at many heights on the way:
            // at 9600 a frame of 256 octets is a fifth of a second, and this
            // page is ninety of them.
            let partway = seen.iter().filter(|&&n| n > 0 && n < page.lines.len()).count();
            assert!(
                partway >= 10,
                "error correction {error_correction}: seen at {partway} heights on the way ({seen:?})"
            );
            assert!(seen.windows(2).all(|w| w[0] < w[1]), "lines went away: {seen:?}");
        }
    }

    #[test]
    fn a_burst_of_noise_costs_a_retransmission_and_not_the_page() {
        // What error correction mode is for. Without it the same burst spoils
        // lines that nobody can ask for again; with it the frames it landed on
        // are asked for, sent again, and the page arrives exactly as it left.
        let page = a_long_page(120);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        let heard = with_a_burst_of_noise(&mut caller, &mut answerer);
        let pps = heard.iter().filter(|f| **f == Frame::Pps).count();
        assert!(caller.error_correction());
        assert!(pps >= 2, "the damaged block was never sent again: {heard:?}");
        let got = answerer.received().unwrap_or_else(|| panic!("no page ({:?})", answerer.trouble()));
        assert_eq!(got.lines, page.lines, "the page was not put right");
    }

    #[test]
    fn without_it_the_same_burst_spoils_the_page() {
        // The control for the test above: the same noise, the same place, and
        // error correction turned off at the answering end.
        let page = a_long_page(120);
        let mut caller = FaxCall::originate(FS, "61399990000", Some(page.clone()));
        let mut answerer = FaxCall::answer(FS, "61388880000").with_error_correction(false);
        with_a_burst_of_noise(&mut caller, &mut answerer);
        assert!(!caller.error_correction(), "used it with a far end that has not got it");
        let got = answerer.received().map(|p| p.lines.clone()).unwrap_or_default();
        assert_ne!(got, page.lines, "the noise missed the page, so this proves nothing");
    }

    #[test]
    fn the_two_ends_learn_each_others_numbers() {
        let mut caller = FaxCall::originate(FS, "61399990000", Some(a_page(4)));
        let mut answerer = FaxCall::answer(FS, "61388880000");
        between(&mut caller, &mut answerer, 40.0);
        assert_eq!(caller.identity(), "61388880000", "the CSI did not arrive");
        assert_eq!(answerer.identity(), "61399990000", "the TSI did not arrive");
    }

    #[test]
    fn a_call_with_no_page_says_so_and_hangs_up() {
        let mut caller = FaxCall::originate(FS, "61399990000", None);
        let mut answerer = FaxCall::answer(FS, "61388880000");
        between(&mut caller, &mut answerer, 40.0);
        assert_eq!(caller.phase(), Phase::Done);
        assert!(answerer.received().is_none(), "a page arrived from nowhere");
    }
}
