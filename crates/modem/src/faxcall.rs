//! A fax call on the line: the tones, V.21, V.27 ter, and T.30 above them.
//!
//! The join between the two halves. [`fax::call`] knows the procedure and
//! nothing about signals; [`datapump::v21`] and [`datapump::v27ter`] know the
//! signals and nothing about the procedure. This puts one on top of the other
//! and gives the result a sample at a time, which is the only thing a line
//! understands.
//!
//! The whole of the join is one question asked once a sample: what should be
//! on the line just now. A fax call answers it with a different thing eight or
//! ten times before a page has moved -- a tone, then 300 bit/s, then silence,
//! then 4800, then silence, then 300 again -- and every one of those changes
//! is a carrier going up or down at both ends.

use datapump::{v21, v27ter};
use fax::call::{Call, Line, Phase, Role};
use fax::page::Page;

/// Which V.27 ter rate a number of bits per second is.
fn rate_of(bits_per_second: u32) -> v27ter::Rate {
    match bits_per_second {
        4800 => v27ter::Rate::R4800,
        _ => v27ter::Rate::R2400,
    }
}

/// A fax call, from either end.
#[derive(Debug)]
pub struct FaxCall {
    call: Call,
    control_tx: v21::Sender,
    control_rx: v21::Receiver,
    fast_tx: v27ter::Transmitter,
    fast_rx: v27ter::Receiver,
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
            fast_tx: v27ter::Transmitter::new(fs),
            fast_rx: v27ter::Receiver::new(fs),
            cng: v21::Tone::new(v21::CNG, fs),
            ced: v21::Tone::new(v21::CED, fs),
            line: Line::Quiet,
        }
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

    /// How far through the page the call has got.
    pub fn progress(&self) -> Option<f64> {
        self.call.progress()
    }

    /// Lines of a page that have arrived.
    pub fn lines_received(&self) -> usize {
        self.call.lines_received()
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
        self.control_rx.carrier() || self.fast_rx.carrier()
    }

    /// Whether the page carrier is the one being listened to.
    ///
    /// A fax call has two receivers and only ever one of them is the one that
    /// matters. Which, decides what a scope should be drawing: the page
    /// carrier has a constellation and the control channel has an eye, and
    /// they are not the same picture at all.
    fn on_the_page_carrier(&self) -> bool {
        matches!(self.line, Line::Fast(_) | Line::FastListen(_))
    }

    /// The point the page carrier last decided on, while it is the one in use.
    pub fn constellation_point(&self) -> Option<(f64, f64)> {
        self.on_the_page_carrier()
            .then(|| self.fast_rx.constellation_point())
    }

    /// The control channel's discriminator, while that is the one in use.
    pub fn discriminator(&self) -> Option<f64> {
        (!self.on_the_page_carrier()).then(|| self.control_rx.discriminator())
    }

    /// One reading per recovered bit of the control channel.
    pub fn take_symbol(&mut self) -> Option<f64> {
        if self.on_the_page_carrier() {
            return None;
        }
        self.control_rx.take_symbol()
    }

    /// Mean distance from the decisions being made, where there are points to
    /// decide between.
    pub fn residual_error(&self) -> Option<f64> {
        self.on_the_page_carrier()
            .then(|| self.fast_rx.residual_error())
    }

    /// That distance as a fraction of the gap between neighbouring points,
    /// where half is the decision boundary.
    pub fn reception(&self) -> Option<f64> {
        self.on_the_page_carrier()
            .then(|| self.fast_rx.residual_error() / self.fast_rx.point_spacing())
    }

    /// How many points the scope should expect.
    pub fn states(&self) -> usize {
        if self.on_the_page_carrier() {
            usize::from(self.fast_rx.rate().phases())
        } else {
            2
        }
    }

    /// Short name for the signal shape, as a faceplate would print it.
    pub fn shape(&self) -> &'static str {
        match (self.on_the_page_carrier(), self.fast_rx.rate()) {
            (false, _) => "2FSK",
            (true, v27ter::Rate::R4800) => "8PSK",
            (true, v27ter::Rate::R2400) => "4PSK",
        }
    }

    /// The modulation carrying the line just now.
    pub fn standard(&self) -> &'static str {
        if self.on_the_page_carrier() {
            "V.27ter"
        } else {
            "V.21"
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
            Line::Fast(_) => self.fast_tx.abort(),
            _ => {}
        }
        match want {
            Line::Control => self.control_tx.set_transmitting(true),
            Line::Fast(rate) => {
                // Always the long turn-on sequence. T.30 leaves the choice to
                // the sender for V.27 ter, and a fax turns the line around
                // between every message, so nothing is remembered from the
                // last burst that a short one could refresh.
                self.fast_tx.start(rate_of(rate), v27ter::Training::Long);
            }
            Line::FastListen(rate) => {
                self.fast_rx.set_rate(rate_of(rate));
                self.fast_rx.restart();
            }
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
            }
            Line::FastListen(_) => {
                self.fast_rx.feed(input);
                let bits = self.fast_rx.take_bits();
                if !bits.is_empty() {
                    self.call.fast_bits(&bits);
                }
                self.call.set_fast_carrier(self.fast_rx.carrier());
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
            Line::Fast(_) => {
                while self.fast_tx.pending_bits() < 32 {
                    match self.call.next_fast_bit() {
                        Some(bit) => self.fast_tx.push_bits(&[bit]),
                        None => break,
                    }
                }
                // Nothing left to hand over and nothing left in the
                // modulator: the burst is over, so take the carrier down the
                // way V.27 ter asks rather than cutting it.
                if self.fast_tx.trained() && self.fast_tx.pending_bits() == 0 {
                    self.fast_tx.stop();
                }
                let idle = !self.fast_tx.is_transmitting();
                (self.fast_tx.next_sample(), idle)
            }
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
    /// carrier is eight points on a circle and what it has is a
    /// constellation. A panel showing neither for the whole of a call is a
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
            shapes.contains(&"2FSK") && shapes.contains(&"8PSK"),
            "the scope was never told what it was drawing: {shapes:?}"
        );
        assert!(eye > 500, "only {eye} readings for the eye");
        assert!(points.len() > 1000, "only {} points", points.len());

        // Eight phases on the unit circle, so everything should land near it.
        let strays = points
            .iter()
            .filter(|(x, y)| {
                let r = (x * x + y * y).sqrt();
                !(0.5..1.6).contains(&r)
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
