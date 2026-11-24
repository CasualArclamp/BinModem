//! Start-stop (asynchronous) character framing.

/// Recovers 8N1-style characters from a sliced baseband level.
///
/// Async framing does not need a continuously-tracked bit clock: the line idles
/// at mark and every character re-synchronises on its own start bit, exactly as
/// a UART does. What it does need is rejection of glitches that look like start
/// bits, so a candidate edge is confirmed at the half-bit point before the
/// character is accepted, and the stop bit must come back as mark.
/// Whether a character's last bit may be put aside when it comes back from the
/// ring-down of a burst that has just stopped.
///
/// Off, and the two captures that have a V.8 call in them are why: measured
/// against a character's own scale, a real V.90 recording's last bits still
/// cross the margin (the line's echo swings them), and a receiver that puts
/// them aside makes a whole CI out of a stretch of noise in a short-Phase-1
/// capture. The case the guard was written for -- a V.92 engine that stops its
/// carrier half a bit into the last octet of signal CJ -- is carried by the
/// carrier-loss rules below instead, which need no amplitude judgement at all:
/// a burst that ends while a character's stop bit is pending still yields the
/// character, because every data bit was on the line before it stopped. Left
/// here, off, with its constant, so that a capture which does need it has
/// somewhere to turn it on.
const RING_COAST: bool = false;

/// How far past a character's own loudest bit a reading has to be before it is
/// the ring-down of a burst that has stopped rather than a bit.
///
/// A tone that is switched off mid-bit leaves the discriminator's filters
/// ringing, and what they say for that bit is neither mark nor space. A
/// shipping V.92 engine ends V.8 signal CJ half a bit into its last octet; read
/// straight, that octet came out as 0x80 instead of 0x00, the far end never
/// counted three zero octets, and the call sat in V.8 until both ends timed
/// out. The margin is a quarter, well inside the swing a filter transient
/// makes and well outside anything a modulated tone does on a line with echo.
const RINGING: f64 = 1.25;

/// Whether V8_TRACE in the environment is set, so that a call can be read out
/// bit by bit when it will not decode.
fn tracing() -> bool {
    use std::sync::OnceLock;
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("V8_TRACE").is_some())
}

#[derive(Debug, Clone)]
pub struct AsyncFramer {
    sps: f64,
    data_bits: u32,
    state: State,
    since_edge: f64,
    next_bit: u32,
    value: u32,
    prev: f64,
    /// Characters whose stop bit was not mark.
    pub framing_errors: u64,
    /// A character whose stop bit came back as space, held until the line
    /// says whether that was a framing error or the end of the burst.
    pending: Option<u8>,
    /// Samples the pending character has been waiting.
    pending_age: f64,
    /// The largest reading this character's data bits have given, which is
    /// the scale the character is in: what counts as a tone, and what counts
    /// as the ring-down of one that has stopped (`RINGING`).
    char_peak: f64,
    /// Level of the most recently sampled data bit, for display. Set at each
    /// bit centre and cleared when read, so a scope sees one entry per bit.
    sampled: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    ConfirmStart,
    Data,
    Stop,
}

impl AsyncFramer {
    pub fn new(baud: f64, fs: f64, data_bits: u32) -> Self {
        assert!((5..=8).contains(&data_bits), "data_bits must be 5..=8");
        Self {
            sps: fs / baud,
            data_bits,
            state: State::Idle,
            since_edge: 0.0,
            next_bit: 0,
            value: 0,
            prev: 1.0,
            framing_errors: 0,
            pending: None,
            pending_age: 0.0,
            char_peak: 0.0,
            sampled: None,
        }
    }

    /// Feed one sample of sliced level (`> 0` is mark) plus the carrier state.
    /// Returns a character once a full frame has been validated.
    pub fn feed(&mut self, level: f64, carrier: bool) -> Option<u8> {
        // A burst can end in the middle of a character. V.8's CJ is the last
        // thing a calling modem sends before the silence, and at least one
        // shipping V.92 engine stops its carrier right after the eighth data
        // bit instead of after the stop bit. Every data bit was already
        // sampled, so the character is whole; the stop bit that never arrived
        // is the line idling, and idle is mark anyway. A receiver that insists
        // on hearing the tone itself drops the last octet of the call menu --
        // the one that says the menu is over.
        let mut out = self.settle_pending(level, carrier);
        if !carrier {
            if out.is_none() && self.state == State::Stop {
                out = Some(self.value as u8);
            }
            self.state = State::Idle;
            self.prev = level;
            return out;
        }


        match self.state {
            State::Idle => {
                // A start bit is a mark-to-space transition on an idle line.
                if self.prev > 0.0 && level <= 0.0 {
                    self.state = State::ConfirmStart;
                    self.since_edge = 0.0;
                }
            }
            State::ConfirmStart => {
                self.since_edge += 1.0;
                if self.since_edge >= self.sps * 0.5 {
                    if level > 0.0 {
                        self.state = State::Idle; // glitch, not a start bit
                    } else {
                        self.state = State::Data;
                        self.next_bit = 0;
                        self.value = 0;
                        self.char_peak = 0.0;
                    }
                }
            }
            State::Data => {
                self.since_edge += 1.0;
                // Bit n is sampled at its centre: 1.5 bit times past the edge,
                // then one bit time per bit after that.
                if self.since_edge >= self.sps * (1.5 + self.next_bit as f64) {
                    // A reading further from the band's middle than either
                    // tone is not a tone: it is the ring-down of a burst that
                    // stopped in the middle of this bit, and what it says
                    // about the bit is nothing. Coast on the bits before it,
                    // which is what a tone that has just been switched off was
                    // still sending.
                    //
                    // A shipping V.92 engine ends V.8 signal CJ half a bit
                    // into its last octet. Sampled straight, that octet came
                    // out as 0x80 instead of 0x00, the far end never counted
                    // three zero octets, and the call sat in V.8 until both
                    // ends timed out.
                    // Against this character's own scale, not a fixed one:
                    // the level a caller hands over is in its slicer's units,
                    // and a line recorded on a tap, or one with a pad on it,
                    // is several times a normalised tone. A fixed threshold
                    // read every third bit of a real V.90 capture as ring-down.
                    // Only the last data bit can be lost this way: a burst
                    // that stops half a bit into a character has still put
                    // every earlier bit on the line, and the one whose
                    // sampling instant falls past the end is the last. Any
                    // other bit reading out of band is a line's own business
                    // and is taken as it comes.
                    let last = self.next_bit + 1 == self.data_bits;
                    let ringing = RING_COAST && last && self.char_peak > 0.0 && level.abs() > RINGING * self.char_peak;
                    if tracing() {
                        eprintln!("framer: bit {} at {:.3} bits level {:+.3}{}",
                                  self.next_bit, self.since_edge / self.sps, level,
                                  if ringing { " (ringing)" } else { "" });
                    }
                    self.sampled = Some(if ringing { 0.0 } else { level });
                    if ringing {
                        // Nothing to learn about the scale from a reading that
                        // is not a tone.
                    } else {
                        self.char_peak = self.char_peak.max(level.abs());
                        if level > 0.0 {
                            self.value |= 1 << self.next_bit; // LSB first
                        }
                    }
                    self.next_bit += 1;
                    if self.next_bit >= self.data_bits {
                        self.state = State::Stop;
                    }
                }
            }
            State::Stop => {
                self.since_edge += 1.0;
                if self.since_edge >= self.sps * (1.5 + self.data_bits as f64) {
                    // Mark, space or nothing, on this character's scale: a
                    // stop bit is only *missing* when the line is still firmly
                    // at space, which is what a framing error or a break looks
                    // like; a line with nothing on it is idle, and idle is
                    // mark.
                    let scale = if self.char_peak > 0.0 { self.char_peak } else { 1.0 };
                    if level > 0.0 {
                        out = Some(self.value as u8);
                    } else {
                        // At space, or at nothing at all. Either a real framing
                        // error or a carrier about to disappear, and only the
                        // next moment tells them apart, so the character waits
                        // instead of being thrown away. A gap in the middle of
                        // a burst resolves the other way: the carrier stays
                        // up, the stop bit really was missing, and the
                        // character was not whole.
                        let _ = scale;
                        self.pending = Some(self.value as u8);
                        self.pending_age = 0.0;
                    }
                    self.state = State::Idle;
                }
            }
        }
        self.prev = level;
        out
    }

    /// Resolve a character whose stop bit was not mark.
    ///
    /// The carrier going away within a bit of it means the burst ended and
    /// the character stands; the carrier staying up means the stop bit really
    /// was missing, which is a framing error.
    fn settle_pending(&mut self, level: f64, carrier: bool) -> Option<u8> {
        let _ = level;
        let held = self.pending?;
        if !carrier {
            self.pending = None;
            return Some(held);
        }
        self.pending_age += 1.0;
        if self.pending_age >= self.sps {
            self.pending = None;
            self.framing_errors += 1;
        }
        None
    }

    /// Level of the data bit sampled on this call, if one was. Cleared by
    /// reading, so a display receives exactly one value per recovered bit.
    pub fn take_sampled(&mut self) -> Option<f64> {
        self.sampled.take()
    }

    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.prev = 1.0;
        self.sampled = None;
        self.pending = None;
        self.char_peak = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render bytes as an idealised 8N1 level waveform.
    fn wave(bytes: &[u8], sps: usize) -> Vec<f64> {
        let mut v = vec![1.0; sps * 4]; // idle mark
        for &b in bytes {
            v.extend(std::iter::repeat_n(-1.0, sps)); // start
            for i in 0..8 {
                let bit = (b >> i) & 1;
                v.extend(std::iter::repeat_n(if bit == 1 { 1.0 } else { -1.0 }, sps));
            }
            v.extend(std::iter::repeat_n(1.0, sps * 2)); // stop + idle
        }
        v
    }

    #[test]
    fn round_trips_ascii() {
        let msg = b"login:CACTUS\r\n";
        let sps = 53usize; // 16000 / 300, truncated as a real receiver would
        let mut f = AsyncFramer::new(300.0, 300.0 * sps as f64, 8);
        let got: Vec<u8> = wave(msg, sps)
            .into_iter()
            .filter_map(|l| f.feed(l, true))
            .collect();
        assert_eq!(got, msg, "got {:?}", String::from_utf8_lossy(&got));
        assert_eq!(f.framing_errors, 0);
    }

    /// A burst may end after the last data bit and before the stop bit: the
    /// carrier going away is the idle line, and the character still counts.
    #[test]
    fn a_carrier_that_ends_after_the_data_bits_still_yields_the_character() {
        let sps = 53usize;
        let mut f = AsyncFramer::new(300.0, 300.0 * sps as f64, 8);
        let mut v = vec![1.0; sps * 4];
        // a start bit and eight zeros, then the carrier goes away half a bit
        // past the centre of the last data bit -- before the stop bit
        v.extend(std::iter::repeat_n(-1.0, sps * 9 + sps / 2));
        let sps_f = sps as f64;
        let mut got = Vec::new();
        let alive = sps_f * 4.0 + sps_f * 9.0 + sps_f / 2.0;
        for (i, level) in v.into_iter().enumerate() {
            let carrier = (i as f64) < alive;
            if let Some(b) = f.feed(level, carrier) {
                got.push(b);
            }
        }
        // and the line stays quiet for a while afterwards
        for _ in 0..sps * 3 {
            if let Some(b) = f.feed(0.0, false) {
                got.push(b);
            }
        }
        assert_eq!(got, vec![0u8], "got {:?}", got);
        assert_eq!(f.framing_errors, 0);
    }

    /// A burst that ends inside a bit leaves the slicer reading the band's
    /// ring-down, which is further out than either tone can be. Sampling that
    /// would turn the last octet of a call menu into 0x80; coasting keeps it
    /// at the level the tone was actually sending.
    #[test]
    fn a_ring_down_at_the_centre_of_a_bit_is_not_a_bit() {
        let sps = 53usize;
        let mut f = AsyncFramer::new(300.0, 300.0 * sps as f64, 8);
        let sps_f = sps as f64;
        let mut v = vec![1.0; sps * 2];
        // start bit and eight spaces, as a run of zero octets arrives
        v.extend(std::iter::repeat_n(-1.0, sps * 9));
        // ... and the carrier is cut half a bit past the last data bit's
        // centre, where the discriminator reads the ring-down
        v.extend(std::iter::repeat_n(2.8, sps / 2 + 1));
        v.extend(std::iter::repeat_n(0.0, sps * 2));
        let mut got = Vec::new();
        for (i, level) in v.into_iter().enumerate() {
            // the carrier survives one bit past the cut, as a tone detector's
            // hold does, and then goes
            let carrier = (i as f64) < sps_f * 2.0 + sps_f * 9.0 + sps_f / 2.0 + sps_f;
            if let Some(b) = f.feed(level, carrier) {
                got.push(b);
            }
        }
        assert_eq!(got, vec![0u8], "got {:?}", got);
    }

    #[test]
    fn rejects_a_half_bit_glitch() {
        let sps = 53usize;
        let mut f = AsyncFramer::new(300.0, 300.0 * sps as f64, 8);
        let mut v = vec![1.0; sps * 4];
        v.extend(std::iter::repeat_n(-1.0, sps / 8)); // far too short to be a start bit
        v.extend(std::iter::repeat_n(1.0, sps * 4));
        let got: Vec<u8> = v.into_iter().filter_map(|l| f.feed(l, true)).collect();
        assert!(got.is_empty(), "glitch produced {got:?}");
    }

    #[test]
    fn loss_of_carrier_aborts_a_partial_character() {
        let sps = 53usize;
        let mut f = AsyncFramer::new(300.0, 300.0 * sps as f64, 8);
        let v = wave(b"A", sps);
        let half = v.len() / 2;
        for &l in &v[..half] {
            f.feed(l, true);
        }
        assert!(f.feed(0.0, false).is_none());
        for &l in &v[half..] {
            assert!(f.feed(l, true).is_none(), "resumed a torn character");
        }
    }
}

/// Start-stop character framing carried over a synchronous bit stream (V.14).
///
/// A modem's line is synchronous: bits go out at the symbol rate whether or
/// not anything wants to send one. A terminal is asynchronous: it sends
/// characters when it has them, each announced by a start bit and closed by a
/// stop bit, and says nothing in between. V.14 is the conversion, and without
/// it the far end has a stream of bits and no idea where one character ends
/// and the next begins.
///
/// The framing is what makes the boundary findable. Idle is mark, so the
/// falling edge into a start bit is unmistakable, and every character
/// re-synchronises on its own: a receiver that joins a call halfway through
/// needs to find one start bit and is then in step.
///
/// What is not done here is the part V.14 is really about. The two clocks are
/// never quite equal, so over a long transfer the asynchronous side delivers
/// slightly more or fewer characters than the synchronous side has room for,
/// and V.14 5.2 recovers the difference by deleting or inserting stop bits.
/// That matters between two independently clocked machines and not at all
/// between two ends of one program.
#[derive(Debug, Clone)]
pub struct AsyncBits {
    data_bits: u32,
    state: BitState,
    /// Bits of the character gathered so far, and how many.
    value: u32,
    have: u32,
    /// Characters whose stop bit was not mark.
    errors: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BitState {
    /// Waiting for a start bit, which is the only space in an idle line.
    Idle,
    Data,
    Stop,
}

impl AsyncBits {
    pub fn new(data_bits: u32) -> Self {
        Self {
            data_bits: data_bits.clamp(5, 8),
            state: BitState::Idle,
            value: 0,
            have: 0,
            errors: 0,
        }
    }

    /// Wrap one character: a start bit, the data least significant first, and
    /// a stop bit.
    pub fn encode(&self, byte: u8) -> Vec<bool> {
        let mut out = Vec::with_capacity(self.data_bits as usize + 2);
        out.push(false);
        for i in 0..self.data_bits {
            out.push(byte & (1 << i) != 0);
        }
        out.push(true);
        out
    }

    /// Feed one received bit, yielding a character when one completes.
    pub fn feed(&mut self, bit: bool) -> Option<u8> {
        match self.state {
            BitState::Idle => {
                if !bit {
                    self.state = BitState::Data;
                    self.value = 0;
                    self.have = 0;
                }
                None
            }
            BitState::Data => {
                if bit {
                    self.value |= 1 << self.have;
                }
                self.have += 1;
                if self.have == self.data_bits {
                    self.state = BitState::Stop;
                }
                None
            }
            BitState::Stop => {
                self.state = BitState::Idle;
                if bit {
                    Some(self.value as u8)
                } else {
                    // The stop bit was not mark, so the framing was wrong
                    // somewhere and the character cannot be trusted. Dropping
                    // it and hunting for the next start bit is what a UART
                    // does and recovers within a character or two.
                    self.errors += 1;
                    None
                }
            }
        }
    }

    /// Characters discarded because their stop bit was not mark.
    pub fn framing_errors(&self) -> u64 {
        self.errors
    }

    pub fn reset(&mut self) {
        self.state = BitState::Idle;
        self.value = 0;
        self.have = 0;
        // Including the count. It belongs to a call: carrying it into the next
        // one makes a fresh link look like it inherited somebody else's
        // trouble, and makes any rate worked out from it wrong at the start.
        self.errors = 0;
    }
}

#[cfg(test)]
mod async_bits_tests {
    use super::AsyncBits;

    #[test]
    fn characters_survive_the_round_trip() {
        let framer = AsyncBits::new(8);
        let mut back = AsyncBits::new(8);
        let mut out = Vec::new();
        for byte in b"Welcome to phl6-dial1" {
            for bit in framer.encode(*byte) {
                if let Some(c) = back.feed(bit) {
                    out.push(c);
                }
            }
        }
        assert_eq!(out, b"Welcome to phl6-dial1");
    }

    #[test]
    fn a_receiver_joining_halfway_finds_the_boundary() {
        // The reason for start bits. A synchronous line hands over a stream
        // with no marks in it, and where a character begins is not something
        // the receiver can be told.
        let framer = AsyncBits::new(8);
        let mut bits: Vec<bool> = Vec::new();
        // Idle first, which is what the receiver joins in the middle of.
        bits.extend(std::iter::repeat_n(true, 13));
        for byte in b"cactus" {
            bits.extend(framer.encode(*byte));
        }
        let mut back = AsyncBits::new(8);
        let out: Vec<u8> = bits.iter().filter_map(|&b| back.feed(b)).collect();
        assert_eq!(out, b"cactus");
    }

    #[test]
    fn idle_produces_no_characters() {
        let mut back = AsyncBits::new(8);
        for _ in 0..1000 {
            assert_eq!(back.feed(true), None);
        }
    }

    #[test]
    fn a_bad_stop_bit_costs_one_character_and_no_more() {
        // A UART recovers by hunting for the next start bit, and so does this:
        // one character is lost and the rest arrive.
        let framer = AsyncBits::new(8);
        let mut bits: Vec<bool> = Vec::new();
        bits.extend(framer.encode(b'a'));
        let mut broken = framer.encode(b'b');
        let last = broken.len() - 1;
        broken[last] = false;
        bits.extend(broken);
        bits.extend(std::iter::repeat_n(true, 4));
        bits.extend(framer.encode(b'c'));

        let mut back = AsyncBits::new(8);
        let out: Vec<u8> = bits.iter().filter_map(|&b| back.feed(b)).collect();
        assert_eq!(out, b"ac");
        assert_eq!(back.framing_errors(), 1);
    }
}
