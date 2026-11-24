//! Compression of the octet stream, and the two ends agreeing on it.
//!
//! The stream the retransmission carries is whatever the terminal wrote.
//! Text, logs and the like are a third or a quarter of their size once
//! compressed, and what is left over is line the next octets can use.
//!
//! Three methods, and each direction uses the best that both ends have:
//!
//! - **LZ**, [`crate::lz`]: a long history and a range coder, in LZMA's
//!   manner. The one to have.
//! - **V.44** and **V.42bis**, as an ordinary modem's error control runs
//!   them, with their largest dictionaries. There because they are there.
//!
//! # Agreeing
//!
//! Nothing can be compressed until the far end is known to be able to
//! undo it, and the far end cannot undo it without knowing exactly which
//! octet of the stream is the first that is compressed. Both are settled
//! with one small frame, the same every time it is sent, saying everything
//! this end knows:
//!
//! ```text
//! version   1
//! can       which methods this end has, a bit each
//! method    what this end's own stream turns into, or 0 for not yet
//! at        the octet of its stream at which it does (8 octets)
//! heard     the far end's `at`, as this end has it (8 octets)
//! ```
//!
//! An end that has heard what the far end can do picks a method and an
//! `at` -- where its stream has got to -- and from then on **holds back
//! everything written** until a frame comes back whose `heard` is that
//! `at`. Only then does the far end certainly know, and what was held goes
//! out compressed. So no compressed octet can arrive before the far end
//! knows where they begin, however many of these frames are lost; the cost
//! is one round trip in which nothing new is sent, once a call.
//!
//! The frames are not retransmitted. They are repeated until they have had
//! their effect, and an end that never hears one -- an older build, which
//! ignores a frame of a kind it does not know -- is sent the stream as it
//! is, for ever.

use crate::lz;

/// The methods, a bit each, the best highest.
pub const V42BIS: u8 = 1;
pub const V44: u8 = 2;
pub const LZ: u8 = 4;
pub const ALL: u8 = V42BIS | V44 | LZ;

const VERSION: u8 = 1;
const NOWHERE: u64 = u64::MAX;

/// Octets in the frame.
pub const FRAME: usize = 19;

/// The name of a method, for saying which is in use.
pub fn name(method: u8) -> &'static str {
    match method {
        LZ => "LZ",
        V44 => "V.44",
        V42BIS => "V.42bis",
        _ => "none",
    }
}

/// The best method in `can`.
fn best(can: u8) -> u8 {
    [LZ, V44, V42BIS]
        .into_iter()
        .find(|m| can & m != 0)
        .unwrap_or(0)
}

fn v42bis_params() -> ec::v42bis::Params {
    ec::v42bis::Params {
        n2: 2048,
        n7: ec::v42bis::OFFERED_N7,
    }
}

fn v44_params() -> ec::v44::Params {
    ec::v44::Params::of(ec::v44::OFFERED_N2, ec::v44::OFFERED_N7)
}

enum Coder {
    V42bis(Box<ec::v42bis::Encoder>),
    V44(Box<ec::v44::Encoder>),
    Lz(Box<lz::Encoder>),
}

impl Coder {
    fn new(method: u8) -> Option<Self> {
        match method {
            V42BIS => Some(Self::V42bis(Box::new(ec::v42bis::Encoder::new(
                v42bis_params(),
            )))),
            V44 => Some(Self::V44(Box::new(ec::v44::Encoder::new(v44_params())))),
            LZ => Some(Self::Lz(Box::default())),
            _ => None,
        }
    }

    /// What `octets` turn into, all of it readable at the far end as soon
    /// as it arrives.
    fn code(&mut self, octets: &[u8], out: &mut Vec<u8>) {
        match self {
            Self::V42bis(encoder) => {
                encoder.encode(octets, out);
                encoder.flush(out);
            }
            Self::V44(encoder) => {
                encoder.encode(octets, out);
                encoder.flush(out);
            }
            Self::Lz(encoder) => encoder.encode(octets, out),
        }
    }
}

enum Uncoder {
    V42bis(Box<ec::v42bis::Decoder>),
    V44(Box<ec::v44::Decoder>),
    Lz(Box<lz::Decoder>),
}

impl Uncoder {
    fn new(method: u8) -> Option<Self> {
        match method {
            V42BIS => Some(Self::V42bis(Box::new(ec::v42bis::Decoder::new(
                v42bis_params(),
            )))),
            V44 => Some(Self::V44(Box::new(ec::v44::Decoder::new(v44_params())))),
            LZ => Some(Self::Lz(Box::default())),
            _ => None,
        }
    }

    fn uncode(&mut self, octets: &[u8], out: &mut Vec<u8>) -> bool {
        match self {
            Self::V42bis(decoder) => decoder.decode(octets, out).is_ok(),
            Self::V44(decoder) => decoder.decode(octets, out).is_ok(),
            Self::Lz(decoder) => decoder.decode(octets, out).is_ok(),
        }
    }
}

/// One end's half of it: what it writes on the way out, what it reads on
/// the way in, and the frame it says so with.
pub struct Squeeze {
    can: u8,
    /// What the far end can do, once it has said.
    theirs_can: Option<u8>,

    /// Octets given to the line so far: where this end's stream has got to.
    put: u64,
    /// The method this end's stream turns into, and at which octet.
    switch: Option<(u8, u64)>,
    /// The far end has said it knows that.
    agreed: bool,
    /// Written since the switch was chosen, and waiting for that.
    held: Vec<u8>,
    coder: Option<Coder>,

    /// Octets taken from the line so far.
    got: u64,
    /// The far end's method, and the octet of its stream it starts at.
    theirs: Option<(u8, u64)>,
    uncoder: Option<Uncoder>,
    /// The far end's stream stopped making sense, and is not read further.
    broken: bool,

    /// There is something new to say, and when the frame was last sent.
    owed: bool,
    said_at: Option<u64>,

    /// Octets before and after, each way, since compression began.
    pub plain_out: u64,
    pub coded_out: u64,
    pub plain_in: u64,
    pub coded_in: u64,
}

impl std::fmt::Debug for Squeeze {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Squeeze")
            .field("can", &self.can)
            .field("theirs_can", &self.theirs_can)
            .field("switch", &self.switch)
            .field("agreed", &self.agreed)
            .field("theirs", &self.theirs)
            .field("broken", &self.broken)
            .finish_non_exhaustive()
    }
}

impl Squeeze {
    /// An end that has the methods in `can`: none, and it neither offers
    /// nor answers.
    pub fn new(can: u8) -> Self {
        Self {
            can: can & ALL,
            theirs_can: None,
            put: 0,
            switch: None,
            agreed: false,
            held: Vec::new(),
            coder: None,
            got: 0,
            theirs: None,
            uncoder: None,
            broken: false,
            owed: false,
            said_at: None,
            plain_out: 0,
            coded_out: 0,
            plain_in: 0,
            coded_in: 0,
        }
    }

    /// Change what this end has. Only before anything has been agreed.
    pub fn offer(&mut self, can: u8) {
        if self.switch.is_none() && self.theirs.is_none() {
            self.can = can & ALL;
        }
    }

    /// The method this end's stream is compressed with, and the far end's,
    /// once each is in use.
    pub fn methods(&self) -> (u8, u8) {
        (
            if self.agreed {
                self.switch.map_or(0, |s| s.0)
            } else {
                0
            },
            self.theirs.map_or(0, |t| t.0),
        )
    }

    /// Whether the far end's stream could not be read.
    pub fn broken(&self) -> bool {
        self.broken
    }

    /// Octets written and held back until the far end is ready for them.
    pub fn held(&self) -> usize {
        self.held.len()
    }

    /// What to give the line for `octets` of the stream.
    pub fn write(&mut self, octets: &[u8]) -> Vec<u8> {
        match (self.switch, self.agreed) {
            (None, _) => {
                self.put += octets.len() as u64;
                octets.to_vec()
            }
            (Some(_), false) => {
                self.held.extend_from_slice(octets);
                Vec::new()
            }
            (Some(_), true) => self.code(octets),
        }
    }

    fn code(&mut self, octets: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(coder) = self.coder.as_mut()
            && !octets.is_empty()
        {
            coder.code(octets, &mut out);
        }
        self.plain_out += octets.len() as u64;
        self.coded_out += out.len() as u64;
        self.put += out.len() as u64;
        out
    }

    /// The stream, out of what the line delivered.
    pub fn read(&mut self, octets: Vec<u8>) -> Vec<u8> {
        if octets.is_empty() || self.broken {
            return Vec::new();
        }
        let from = self.got;
        self.got += octets.len() as u64;
        let Some((_, at)) = self.theirs else {
            return octets;
        };
        // Up to `at` the stream is as it was written.
        let plain = at.saturating_sub(from).min(octets.len() as u64) as usize;
        let mut out = octets[..plain].to_vec();
        let coded = &octets[plain..];
        if !coded.is_empty() {
            let before = out.len();
            let read = self
                .uncoder
                .as_mut()
                .is_some_and(|uncoder| uncoder.uncode(coded, &mut out));
            if !read {
                self.broken = true;
                out.truncate(before);
            }
            self.coded_in += coded.len() as u64;
            self.plain_in += (out.len() - before) as u64;
        }
        out
    }

    /// The frame, if it is time to send it: `now` and `fs` in samples.
    pub fn say(&mut self, now: u64, fs: f64) -> Option<Vec<u8>> {
        if self.can == 0 {
            return None;
        }
        let since = self.said_at.map_or(u64::MAX, |at| now.saturating_sub(at));
        // Until the far end has been heard, for half a minute and no
        // longer: one that has not answered by then is not going to.
        let unanswered = self.theirs_can.is_none() && (now as f64) < 30.0 * fs;
        let waiting = self.switch.is_some() && !self.agreed;
        let due = (unanswered || waiting) && since as f64 > 0.25 * fs;
        // Something new is said at once, but not twice in a tenth of a
        // second, or two ends would do nothing else.
        if !(due || (self.owed && since as f64 > 0.1 * fs)) {
            return None;
        }
        self.owed = false;
        self.said_at = Some(now);
        let mut body = vec![VERSION, self.can, self.switch.map_or(0, |s| s.0)];
        body.extend_from_slice(&self.switch.map_or(NOWHERE, |s| s.1).to_le_bytes());
        body.extend_from_slice(&self.theirs.map_or(NOWHERE, |t| t.1).to_le_bytes());
        Some(body)
    }

    /// A frame from the far end. What comes back is what was being held
    /// and can now go to the line.
    pub fn hear(&mut self, body: &[u8]) -> Vec<u8> {
        if self.can == 0 || body.len() < FRAME || body[0] != VERSION {
            return Vec::new();
        }
        let (can, method) = (body[1] & ALL, body[2]);
        let at = u64::from_le_bytes(body[3..11].try_into().unwrap_or([0xFF; 8]));
        let heard = u64::from_le_bytes(body[11..19].try_into().unwrap_or([0xFF; 8]));

        if self.theirs_can.is_none() {
            self.theirs_can = Some(can);
            self.owed = true;
            // What this end's stream will be, and from where: here.
            let method = best(self.can & can);
            if method != 0 {
                self.switch = Some((method, self.put));
                self.coder = Coder::new(method);
            }
        }
        if at != NOWHERE
            && self.theirs.is_none()
            && method & self.can != 0
            && method.is_power_of_two()
        {
            // Nothing from there on has arrived, or the far end did not
            // hold it back as it has to.
            if at >= self.got {
                self.theirs = Some((method, at));
                self.uncoder = Uncoder::new(method);
            } else {
                self.broken = true;
            }
            self.owed = true;
        }
        // The far end still waiting to be told is told again.
        if at != NOWHERE && self.theirs.is_some_and(|t| t.1 == at) && heard == NOWHERE {
            self.owed = true;
        }
        if let Some((_, mine)) = self.switch
            && !self.agreed
            && heard == mine
        {
            self.agreed = true;
            let held = std::mem::take(&mut self.held);
            return self.code(&held);
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 8000.0;

    /// Two ends and frames between them that can be lost, with a line each
    /// way that delivers what it was given in order.
    struct Pair {
        a: Squeeze,
        b: Squeeze,
        now: u64,
        to_b: Vec<u8>,
        to_a: Vec<u8>,
        at_b: Vec<u8>,
        at_a: Vec<u8>,
        /// Lose every frame while this is true.
        deaf: bool,
        frames: usize,
    }

    impl Pair {
        fn new(a: u8, b: u8) -> Self {
            Self {
                a: Squeeze::new(a),
                b: Squeeze::new(b),
                now: 0,
                to_b: vec![],
                to_a: vec![],
                at_b: vec![],
                at_a: vec![],
                deaf: false,
                frames: 0,
            }
        }

        /// A tenth of a second: frames each way, then the lines.
        fn tick(&mut self) {
            self.now += (FS / 10.0) as u64 + 1;
            let (from_a, from_b) = (self.a.say(self.now, FS), self.b.say(self.now, FS));
            self.frames += usize::from(from_a.is_some()) + usize::from(from_b.is_some());
            if !self.deaf {
                if let Some(frame) = from_a {
                    let ready = self.b.hear(&frame);
                    self.to_a.extend(ready);
                }
                if let Some(frame) = from_b {
                    let ready = self.a.hear(&frame);
                    self.to_b.extend(ready);
                }
            }
            let (to_b, to_a) = (
                std::mem::take(&mut self.to_b),
                std::mem::take(&mut self.to_a),
            );
            self.at_b.extend(self.b.read(to_b));
            self.at_a.extend(self.a.read(to_a));
        }

        fn write_a(&mut self, octets: &[u8]) {
            let out = self.a.write(octets);
            self.to_b.extend(out);
        }

        fn write_b(&mut self, octets: &[u8]) {
            let out = self.b.write(octets);
            self.to_a.extend(out);
        }
    }

    fn text(len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| {
                b"the line carries what the terminal wrote, and most of that is text. "[i % 68]
            })
            .collect()
    }

    #[test]
    fn two_ends_agree_while_the_stream_runs_and_nothing_is_lost_or_out_of_place() {
        for (a, b, each) in [
            (ALL, ALL, LZ),
            (ALL, V44 | V42BIS, V44),
            (V42BIS, ALL, V42BIS),
            (LZ, LZ, LZ),
        ] {
            let mut pair = Pair::new(a, b);
            let (mut sent_b, mut sent_a) = (Vec::new(), Vec::new());
            for i in 0..80 {
                // Written all the time, before, during and after.
                let piece = text(300 + i * 7);
                pair.write_a(&piece);
                sent_b.extend_from_slice(&piece);
                pair.write_b(&piece[..piece.len() / 2]);
                sent_a.extend_from_slice(&piece[..piece.len() / 2]);
                pair.tick();
            }
            for _ in 0..5 {
                pair.tick();
            }
            assert!(
                pair.at_b == sent_b,
                "{a}/{b}: {} of {}",
                pair.at_b.len(),
                sent_b.len()
            );
            assert!(
                pair.at_a == sent_a,
                "{a}/{b}: {} of {}",
                pair.at_a.len(),
                sent_a.len()
            );
            assert_eq!(pair.a.methods(), (each, each), "{a}/{b}");
            assert_eq!(pair.b.methods(), (each, each), "{a}/{b}");
            assert!(
                pair.a.coded_out * 2 < pair.a.plain_out,
                "{a}/{b}: {} for {}",
                pair.a.coded_out,
                pair.a.plain_out
            );
            assert!(!pair.a.broken() && !pair.b.broken());
        }
    }

    #[test]
    fn frames_that_are_lost_cost_time_and_nothing_else() {
        let mut pair = Pair::new(ALL, ALL);
        let mut sent = Vec::new();
        for i in 0..120 {
            // Deaf most of the time, at every stage of agreeing.
            pair.deaf = i % 7 != 0;
            let piece = text(200);
            pair.write_a(&piece);
            sent.extend_from_slice(&piece);
            pair.tick();
        }
        pair.deaf = false;
        for _ in 0..10 {
            pair.tick();
        }
        assert!(pair.at_b == sent, "{} of {}", pair.at_b.len(), sent.len());
        assert_eq!(pair.a.methods().0, LZ);
        assert_eq!(pair.a.held(), 0);
    }

    #[test]
    fn an_end_with_nothing_or_a_far_end_that_never_answers_carries_the_stream_as_it_is() {
        // Nothing in common: both speak, neither compresses.
        let mut pair = Pair::new(V44, LZ);
        pair.write_a(b"plain");
        for _ in 0..20 {
            pair.tick();
            pair.write_a(b"plain");
        }
        pair.tick();
        assert_eq!(pair.at_b.len(), 5 * 21);
        assert_eq!(pair.a.methods(), (0, 0));

        // An older build: it says nothing and hears nothing.
        let mut pair = Pair::new(ALL, 0);
        for _ in 0..400 {
            pair.write_a(b"plain");
            pair.tick();
        }
        assert_eq!(pair.at_b.len(), 5 * 400);
        // And is not asked for ever.
        let before = pair.frames;
        for _ in 0..100 {
            pair.tick();
        }
        assert_eq!(pair.frames, before);
    }

    #[test]
    fn once_agreed_the_two_ends_have_nothing_more_to_say() {
        let mut pair = Pair::new(ALL, ALL);
        for _ in 0..30 {
            pair.tick();
        }
        assert_eq!(pair.a.methods(), (LZ, LZ));
        let before = pair.frames;
        for _ in 0..100 {
            pair.write_a(b"and the stream goes on");
            pair.tick();
        }
        assert_eq!(pair.frames, before);
    }
}
