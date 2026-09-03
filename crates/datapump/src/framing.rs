//! Start-stop (asynchronous) character framing.

/// Recovers 8N1-style characters from a sliced baseband level.
///
/// Async framing does not need a continuously-tracked bit clock: the line idles
/// at mark and every character re-synchronises on its own start bit, exactly as
/// a UART does. What it does need is rejection of glitches that look like start
/// bits, so a candidate edge is confirmed at the half-bit point before the
/// character is accepted, and the stop bit must come back as mark.
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
            sampled: None,
        }
    }

    /// Feed one sample of sliced level (`> 0` is mark) plus the carrier state.
    /// Returns a character once a full frame has been validated.
    pub fn feed(&mut self, level: f64, carrier: bool) -> Option<u8> {
        if !carrier {
            self.state = State::Idle;
            self.prev = level;
            return None;
        }

        let mut out = None;
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
                    }
                }
            }
            State::Data => {
                self.since_edge += 1.0;
                // Bit n is sampled at its centre: 1.5 bit times past the edge,
                // then one bit time per bit after that.
                if self.since_edge >= self.sps * (1.5 + self.next_bit as f64) {
                    self.sampled = Some(level);
                    self.value |= u32::from(level > 0.0) << self.next_bit; // LSB first
                    self.next_bit += 1;
                    if self.next_bit >= self.data_bits {
                        self.state = State::Stop;
                    }
                }
            }
            State::Stop => {
                self.since_edge += 1.0;
                if self.since_edge >= self.sps * (1.5 + self.data_bits as f64) {
                    if level > 0.0 {
                        out = Some(self.value as u8);
                    } else {
                        self.framing_errors += 1;
                    }
                    self.state = State::Idle;
                }
            }
        }
        self.prev = level;
        out
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
