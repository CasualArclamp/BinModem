//! What a real V.92 call did, read off a recording.
//!
//! `tests/vectors/v92-56k.wav` is the same 2005 Conexant softmodem and the
//! same 56k server as `v90-56k.wav`, dialled again with `AT+MS` set to V.92,
//! both directions summed on one tap. The vectors README calls it "V.34-style
//! startup", and that turns out to be wrong about the beginning and right
//! about the end.
//!
//! **There is no V.8 in it.** No CM, no JM, no CJ: this call opened with
//! V.92's *short* Phase 1 (9.2), and with the V.8 bis flavour of it -- the
//! QC2a/QCA2d pair of Tables 3 and 14, HDLC frames on the two V.21 channels,
//! not the V.8-framed QC1a that `v8::quick` codes. What the clip holds, in
//! order, is the second half of Figure 5/V.92:
//!
//! ```text
//! analogue (calling): .. QC2a on V.21(H) ....................... TONEq .. | INFO0a
//! digital (answering): ........ QCA2d on V.21(L) .. 75 ms .. QTS QTS\ ANSpcm .... | INFO0d
//! ```
//!
//! The CRe that opened it, and the first part of QC2a, are before the clip
//! starts. Everything after that is here and is checked below, down to the
//! 75 ms silence, the 96 ms of QTS, its closing reversal, ANSpcm's 451.5 ms
//! reversals and the one second from ANSpcm to TONEq.
//!
//! Then Phase 2 runs in full -- both INFO0s say V.92, and the server's INFO1d
//! says the channel carries PCM upstream -- and the analogue modem asks for
//! **V.90 data mode anyway**: Table 10/V.90, 8000 down and V.34 at 3200 up,
//! bit for bit the INFO1a of the V.90 recording. So the one V.92 capture this
//! project has contains no PCM upstream to learn from. That is worth knowing
//! before Phase 3 is written, and it is why the last test here is ignored.
//!
//! Nothing in this file adjusts any reading to fit the capture. Where the
//! capture and a clause differ -- the analogue modem takes 93 ms between
//! TONEq and INFO0a where 9.2.1.3 asks for 75 +/- 5 -- the measurement is
//! recorded and the clause is left alone.

use datapump::v34::dpsk::{Receiver, Side};
use datapump::v34::info::{Info, PcmFlags, SymbolRate};
use datapump::Bell103Rx;
use ec::hdlc::{Decoder as Hdlc, Fcs};
use v8::quick::{AnspcmLevel, BitWatcher, Qc, Uqts};
use v8::{Decoder, Heard, Menu};

const VECTOR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/v92-56k.wav");
/// The V.90 recording, used only as a positive control: the same code that
/// finds no menus in the V.92 file finds five CMs and three JMs in this one.
const V90_VECTOR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/v90-56k.wav");

/// V.21 signalling rate (8.2/V.92, V.21).
const BAUD: f64 = 300.0;

/// The third harmonic of the 8 kHz frame rate, 8000/6.
///
/// QTS repeats `{+V, +0, +V, -V, -0, -V}` (8.3.6) and Ru repeats
/// `{+LU, +LU, +LU, -LU, -LU, -LU}` (8.5.5); both satisfy x[n+3] = -x[n], so
/// both put their energy at 8000/6 Hz and at 4000 Hz and nowhere else. One
/// detector therefore finds either of them, which is what makes the ignored
/// Ru test below worth writing: QTS proves the detector works.
const F_FRAME_3RD: f64 = 8000.0 / 6.0;

/// ANSpcm's tone: 79 cycles in a 301-symbol period at 8000 symbol/s (8.3.1).
const F_ANSPCM: f64 = 79.0 / 301.0 * 8000.0;

/// TONEq (8.2.5), which is also the V.21(L) mark frequency.
const F_TONEQ: f64 = 980.0;

/// How far apart ANSpcm's phase reversals are: 3612 symbols (8.3.1).
const ANSPCM_REVERSAL: f64 = 3612.0 / 8000.0;

/// How long QTS runs before QTS\ turns it over: 768 symbols (8.3.6).
const QTS_LENGTH: f64 = 768.0 / 8000.0;

/// The amplitude above which a tone is taken to be present, on `line::wav`'s
/// scale, where full scale is 1.
///
/// The quietest signal measured below is QTS at 0.021; the loudest thing that
/// is not one of these tones is the V.21 data's own leakage into a
/// neighbouring bin, at 0.010. This sits between them. It is a property of
/// this recording and of nothing else.
const TONE_PRESENT: f64 = 0.012;

/// The amplitude above which QTS's 4 kHz partner is taken to be present.
///
/// It is the stronger of the two components at the codec -- the six-symbol
/// pattern's third harmonic is twice its fundamental -- and much the weaker
/// here, because 4 kHz is where the reconstruction filter stops. Measured at
/// 0.0042 against the fundamental's 0.021.
const FOUR_KHZ_PRESENT: f64 = 0.002;

/// The window each tone measurement integrates over.
///
/// 6 ms is eight whole cycles of [`F_FRAME_3RD`], which keeps QTS's own
/// measurement honest, and it bounds every edge time below to +/-6 ms.
const TONE_WINDOW: f64 = 0.006;

/// How many windows below the threshold a run of a tone may contain.
///
/// Two, which is the most a 180 degree reversal can empty when the windows
/// overlap by half.
const RUN_GAP: u32 = 2;

#[derive(Debug)]
struct Capture {
    fs: f64,
    x: Vec<f32>,
}

impl Capture {
    fn read(path: &str) -> Self {
        let wav = line::wav::read(path).expect("could not read the vector");
        Self { fs: f64::from(wav.sample_rate), x: wav.channel(0) }
    }

    fn at(&self, t: f64) -> usize {
        ((t * self.fs).round() as usize).min(self.x.len())
    }

    /// Amplitude and phase of `f` over `len` seconds from `from`, referred to
    /// the start of the recording so that two windows can be compared.
    ///
    /// The reference oscillator is stepped by a complex rotation rather than
    /// by a sine and a cosine per sample, because the V.21 slicer below asks
    /// for this fifty-three times per bit and the file is twenty-three seconds
    /// long. Its starting angle is taken modulo a turn first, which is what
    /// keeps two windows a second apart comparable in phase at all.
    fn tone(&self, f: f64, from: f64, len: f64) -> (f64, f64) {
        let (a, b) = (self.at(from), self.at(from + len));
        if b <= a {
            return (0.0, 0.0);
        }
        let turns = (f * a as f64 / self.fs).rem_euclid(1.0) * std::f64::consts::TAU;
        let step = std::f64::consts::TAU * f / self.fs;
        let (mut c, mut s) = (turns.cos(), -turns.sin());
        let (dc, ds) = (step.cos(), -step.sin());
        let (mut re, mut im) = (0.0, 0.0);
        for &sample in &self.x[a..b] {
            re += f64::from(sample) * c;
            im += f64::from(sample) * s;
            (c, s) = (c * dc - s * ds, c * ds + s * dc);
        }
        let n = (b - a) as f64;
        (2.0 * (re * re + im * im).sqrt() / n, im.atan2(re))
    }

    /// When something first rises above [`TONE_PRESENT`] in `from..to`, and
    /// when that first run of it ends.
    ///
    /// The run survives up to [`RUN_GAP`] windows below the threshold, because
    /// a phase reversal empties the window that straddles it and both QTS and
    /// ANSpcm turn over in the middle of their own runs. Taking the *first*
    /// run and not the outermost pair matters at the other end: the 2100 Hz
    /// bin fills up again in Phase 2, half a second after ANSpcm stopped.
    fn run(&self, from: f64, to: f64, amplitude: impl Fn(f64) -> f64) -> Option<(f64, f64)> {
        let hop = TONE_WINDOW / 2.0;
        let (mut on, mut off, mut missed) = (None, from, 0);
        let mut t = from;
        while t + TONE_WINDOW <= to {
            if amplitude(t) > TONE_PRESENT {
                on.get_or_insert(t);
                off = t + TONE_WINDOW;
                missed = 0;
            } else if on.is_some() {
                missed += 1;
                if missed > RUN_GAP {
                    break;
                }
            }
            t += hop;
        }
        on.map(|first| (first, off))
    }

    /// The first run of one tone.
    fn span(&self, f: f64, from: f64, to: f64) -> Option<(f64, f64)> {
        self.run(from, to, |t| self.tone(f, t, TONE_WINDOW).0)
    }

    /// The first run of a V.21 carrier, which is whichever of its two tones
    /// the modem happens to be sending.
    fn carrier(&self, tones: (f64, f64), from: f64, to: f64) -> Option<(f64, f64)> {
        self.run(from, to, |t| self.tone(tones.0, t, TONE_WINDOW).0 + self.tone(tones.1, t, TONE_WINDOW).0)
    }

    /// Every place the phase of `f` turns over inside `from..to`.
    ///
    /// Reported as the boundary between the last window of the old phase and
    /// the first of the new, so each time is good to a window.
    fn reversals(&self, f: f64, from: f64, to: f64, window: f64) -> Vec<f64> {
        let mut out = Vec::new();
        let mut previous: Option<(f64, f64)> = None;
        let mut t = from;
        while t + window <= to {
            let (amplitude, phase) = self.tone(f, t, window);
            if amplitude > TONE_PRESENT {
                if let Some((was, at)) = previous {
                    let mut turn = phase - was;
                    while turn > std::f64::consts::PI {
                        turn -= std::f64::consts::TAU;
                    }
                    while turn < -std::f64::consts::PI {
                        turn += std::f64::consts::TAU;
                    }
                    if turn.abs() > 100.0_f64.to_radians() {
                        out.push((at + t) / 2.0);
                    }
                }
                previous = Some((phase, t + window));
            }
            t += window;
        }
        out
    }

    /// The V.21 bits of one channel in `from..to`, and the time of the first.
    ///
    /// Non-coherent detection: each bit is whichever of the two tones carries
    /// the more energy over its own bit time. There is no timing loop -- the
    /// bit phase is chosen once, as the offset whose decisions are on average
    /// the least marginal, which is enough for a burst of a few hundred bits
    /// from a modem whose clock is as good as this one's.
    fn v21(&self, tones: (f64, f64), from: f64, to: f64) -> (f64, Vec<bool>) {
        let (space, mark) = tones;
        let sps = self.fs / BAUD;
        let count = ((to - from) * BAUD) as usize;
        let start = self.at(from);
        let mut best = (f64::NEG_INFINITY, 0usize, Vec::new());
        for offset in 0..sps as usize {
            let (mut bits, mut margin) = (Vec::with_capacity(count), 0.0);
            for i in 0..count {
                let at = (start + offset) as f64 / self.fs + i as f64 / BAUD;
                let m = self.tone(mark, at, 1.0 / BAUD).0;
                let s = self.tone(space, at, 1.0 / BAUD).0;
                bits.push(m > s);
                margin += (m - s).abs() / (m + s + f64::EPSILON);
            }
            if margin > best.0 {
                best = (margin, offset, bits);
            }
        }
        ((start + best.1) as f64 / self.fs, best.2)
    }
}

fn capture() -> Capture {
    Capture::read(VECTOR)
}

/// Every V.8 message heard in one V.21 channel of a recording.
fn menus(path: &str, tones: (f64, f64)) -> Vec<(f64, Heard)> {
    let capture = Capture::read(path);
    let mut rx = Bell103Rx::with_tones(tones.0, tones.1, capture.fs);
    let mut decoder = Decoder::new();
    let mut out = Vec::new();
    for (i, &s) in capture.x.iter().enumerate() {
        if let Some(octet) = rx.feed(f64::from(s))
            && let Some(heard) = decoder.feed(octet)
        {
            out.push((i as f64 / capture.fs, heard));
        }
    }
    out
}

fn sequences() -> Vec<(f64, Side, Info)> {
    let capture = capture();
    let mut out = Vec::new();
    for side in [Side::Call, Side::Answer] {
        let mut rx = Receiver::new(side, capture.fs);
        for (i, &s) in capture.x.iter().enumerate() {
            if let Some(info) = rx.feed(f64::from(s)) {
                out.push((i as f64 / capture.fs, side, info));
            }
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// One of the four V.8 bis-framed quick-connect messages, as its two
/// identification octets say it is (Tables 3, 5, 12 and 14/V.92).
///
/// `v8::quick` codes the V.8-framed QC1a family and says so; these are V.8 bis
/// messages, which this modem has no V.8 bis to send, so the two octets are
/// unpicked here. The fields themselves are V92-07's: U_QTS and LM are the
/// same codes in both framings.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Quick2 {
    /// Identification bit 14: an acknowledge rather than a request.
    acknowledge: bool,
    /// Identification bit 15: the digital modem is speaking.
    digital: bool,
    /// Identification bit 13, P: "calls for LAPM protocol" (9.2.5).
    lapm: bool,
    /// Bits 8:11, U_QTS, from an analogue modem.
    uqts: Option<Uqts>,
    /// Bits 8:9, LM, from a digital modem.
    level: Option<AnspcmLevel>,
}

impl Quick2 {
    /// Read the two-octet identification field.
    ///
    /// V.8 bis sends bit 1 of an octet first, so identification bit n is
    /// octet 1 bit n+1 for n = 0..7 and octet 2 bit n-7 for n = 8..15. Octet 1
    /// is the message type `1011` (V.8 bis Table 3, "Defined in ITU-T V.92")
    /// and a revision receivers ignore, so only its low nibble is checked.
    fn from_field(field: &[u8]) -> Option<Self> {
        let (&first, &second) = (field.first()?, field.get(1)?);
        if first & 0x0f != 0x0d {
            return None;
        }
        let bit = |n: u32| second & (1 << (n - 1)) != 0;
        let digital = bit(8);
        // Bits 10:12 of the identification field are "reserved for the ITU"
        // in the digital layouts and bit 12 in the analogue ones; the
        // analogue layouts carry Y and Z where the digital ones have two of
        // those zeros, which is why only bit 5 is common to both.
        if bit(5) || (digital && (bit(3) || bit(4))) {
            return None;
        }
        let pattern = |n: u32| u8::from(bit(n));
        Some(Self {
            acknowledge: bit(7),
            digital,
            lapm: bit(6),
            uqts: (!digital)
                .then(|| Uqts::from_pattern((pattern(1) << 3) | (pattern(2) << 2) | (pattern(3) << 1) | pattern(4)))
                .flatten(),
            level: digital.then(|| AnspcmLevel::from_pattern((pattern(1) << 1) | pattern(2))).flatten(),
        })
    }

    /// The Recommendation's name for it.
    fn name(&self) -> &'static str {
        match (self.acknowledge, self.digital) {
            (false, false) => "QC2a",
            (true, false) => "QCA2a",
            (false, true) => "QC2d",
            (true, true) => "QCA2d",
        }
    }
}

/// Every well-formed HDLC frame in a V.21 burst, each with the time its first
/// closing flag finished.
///
/// V.8 bis 7.2.3-7.2.9 frames a message exactly as LAPM frames one -- flags,
/// zero-bit insertion, the ISO/IEC 3309 16-bit FCS -- so `ec::hdlc` reads it
/// unchanged, and hands the frame over on that closing flag's own last bit.
/// A frame that fails its check sequence is dropped here without comment,
/// because the only thing this file wants to say about such a frame is that
/// it is not one of the two below.
fn hdlc(bits: &[bool], first_at: f64) -> Vec<(f64, Vec<u8>)> {
    let mut decoder = Hdlc::new(Fcs::Bits16);
    bits.iter()
        .enumerate()
        .filter_map(|(i, &bit)| match decoder.feed(bit) {
            Some(Ok(frame)) => Some((first_at + (i + 1) as f64 / BAUD, frame)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_v8_menus_never_come_because_this_call_used_short_phase_1() {
    // V.92 9.2: short Phase 1 "replaces the CM/JM exchange". There is no call
    // function octet, no modulation octets, no PCM availability category and
    // no CJ -- so the question the plan asked of this capture, what the V.8
    // menus offered, has no answer in it.
    for (label, tones) in [("V.21(L)", datapump::v8::LOW), ("V.21(H)", datapump::v8::HIGH)] {
        let heard = menus(VECTOR, tones);
        assert!(heard.is_empty(), "{label} carried {heard:?}");
    }
    // And the absence is the line's, not the decoder's: the same two passes
    // over the V.90 recording of the same modem and the same server find the
    // five CMs and three JMs that `v90_vector.rs` also finds. Only the V.8
    // phase is looked at, because a CM carries no check sequence and one
    // assembles itself out of the V.90 data mode at 23.7 s of that recording.
    let phase1 = |tones| -> Vec<Menu> {
        menus(V90_VECTOR, tones)
            .into_iter()
            .filter_map(|(at, heard)| match heard {
                Heard::Cm(menu) if at < 10.0 => Some(menu),
                _ => None,
            })
            .collect()
    };
    let (cm, jm) = (phase1(datapump::v8::LOW), phase1(datapump::v8::HIGH));
    assert_eq!(cm.len(), 5, "V.90's CMs");
    assert_eq!(jm.len(), 3, "V.90's JMs");
    assert_eq!(cm[0].pcm, Some(v8::Pcm::ANALOGUE));
    assert_eq!(jm[0].pcm, Some(v8::Pcm { analogue: false, digital: true, v91: false }));
}

#[test]
fn a_qc1a_before_the_cm_is_looked_for() {
    // Found: **no QC1a, and no QC of any V.8 framing at all.** The plan asked
    // for the V.8-framed sequence of Table 2 on V.21(L), because that is the
    // one an ANSam opening leads to (Figure 3). This call opened the other
    // way, with CRe, and Figure 5 puts QC2a -- a V.8 bis message -- on
    // V.21(H) instead. `v8::quick` codes only the four V.8-framed sequences
    // and says so in its module comment, so nothing it can recognise was
    // ever on this line.
    //
    // Both channels are swept, not just the one the plan named, so that the
    // answer does not depend on having guessed the opening right.
    let capture = capture();
    for (label, tones) in [("V.21(L)", datapump::v8::LOW), ("V.21(H)", datapump::v8::HIGH)] {
        let (_, bits) = capture.v21(tones, 0.0, 6.0);
        let mut watcher = BitWatcher::new();
        let found: Vec<Qc> = bits.iter().filter_map(|&bit| watcher.feed(bit)).collect();
        assert!(found.is_empty(), "{label} carried {found:?}");
    }
    // The V.92 synchronisation pattern itself is on this line, though, and it
    // is worth knowing where: once, inside QC2a's own HDLC body, where the
    // identification octets and the frame check happen to alternate. A
    // watcher that hunted the synchronisation alone would report a quick
    // connect there. `BitWatcher` anchors the whole sixty-bit window instead,
    // and the ten ONEs Table 2 puts in front of every QC are what save it.
    let (at, bits) = capture.v21(datapump::v8::HIGH, 1.0, 1.7);
    let sync = [false, true, false, true, false, true, false, true, false, true];
    let hits: Vec<usize> =
        bits.windows(sync.len()).enumerate().filter_map(|(i, w)| (w == sync).then_some(i)).collect();
    let times: Vec<f64> = hits.iter().map(|&i| at + i as f64 / BAUD).collect();
    assert_eq!(hits.len(), 1, "V.92 synchronisations on V.21(H) at {times:.3?}");
    assert!(hits[0] >= 10 && !bits[hits[0] - 10..hits[0]].iter().all(|b| *b), "ten ONEs in front of it");
}

#[test]
fn the_quick_connect_is_the_v8_bis_pair_qc2a_and_qca2d() {
    let capture = capture();

    // QC2a, from the analogue calling modem, on V.21(H) (8.2.2, Table 3).
    // The clip opens part-way through its 100 ms mark preamble, so the frame
    // itself is all that can be checked.
    let (at, bits) = capture.v21(datapump::v8::HIGH, 1.0, 1.7);
    let frames = hdlc(&bits, at);
    assert_eq!(frames.len(), 1, "V.21(H) frames: {frames:?}");
    let (qc2a_end, ref field) = frames[0];
    println!("QC2a closes at {qc2a_end:.3}: {field:02x?}");
    assert_eq!(field.as_slice(), &[0x2d, 0x25], "QC2a identification octets");
    let qc2a = Quick2::from_field(field).expect("QC2a does not read as an identification field");
    assert_eq!(qc2a.name(), "QC2a");
    assert!(qc2a.lapm, "P: the analogue modem asks for LAPM");
    // WXYZ = 1010, which Table 2 gives as Ucode 79. The same modem asks for
    // UINFO 78 in its INFO1a, and asked for 78 in the V.90 recording too.
    assert_eq!(qc2a.uqts.and_then(Uqts::ucode), Some(79));

    // QCA2d, from the digital answering modem, on V.21(L) (8.3.5, Table 14).
    let (at, bits) = capture.v21(datapump::v8::LOW, 1.5, 2.05);
    let frames = hdlc(&bits, at);
    assert_eq!(frames.len(), 1, "V.21(L) frames: {frames:?}");
    let (qca2d_end, ref field) = frames[0];
    println!("QCA2d closes at {qca2d_end:.3}: {field:02x?}");
    // Bit for bit the vector `spec-phase1-procedures.md` 3.3 derives for
    // QCA2d with P = 1 and LM = 01, worked out from the Recommendation
    // before this file was read.
    assert_eq!(field.as_slice(), &[0x2d, 0xe2], "QCA2d identification octets");
    let qca2d = Quick2::from_field(field).expect("QCA2d does not read as an identification field");
    assert_eq!(qca2d.name(), "QCA2d");
    assert!(qca2d.lapm, "P: the digital modem agrees to LAPM, so 9.2.5 bypasses ODP/ADP");
    assert_eq!(qca2d.level, Some(AnspcmLevel::Minus12), "LM = 01");
    // The order settles which figure this is: QC2a first, on V.21(H) with the
    // analogue modem's U_QTS, then QCA2d on V.21(L) with the digital modem's
    // LM. That is Figure 5 -- analogue calling, CRe answering -- and it is
    // also why the 75 ms of silence below follows the V.21(L) burst and not
    // the V.21(H) one.
    assert!(qca2d_end > qc2a_end, "QCA2d closed at {qca2d_end:.3}, before QC2a's {qc2a_end:.3}");
}

#[test]
fn qts_follows_qca2d_by_seventy_five_milliseconds_and_runs_for_768_symbols() {
    let capture = capture();

    // The end of QCA2d: where the V.21(L) carrier stops. Measured at 1.946 s.
    let (_, qca2d_off) = capture.carrier(datapump::v8::LOW, 1.7, 2.0).expect("no V.21(L) burst");

    // QTS: a tone at 8000/6 with a companion at 4000 Hz, which is the whole
    // spectrum of a six-symbol pattern obeying x[n+3] = -x[n]. The 4 kHz
    // partner is the stronger of the two at the codec and the weaker here,
    // because it sits on the reconstruction filter's edge.
    let (qts_on, qts_off) = capture.span(F_FRAME_3RD, 2.0, 2.2).expect("no QTS");
    let (four_k, _) = capture.tone(4000.0, qts_on + 0.02, 0.05);
    let (third, _) = capture.tone(F_FRAME_3RD, qts_on + 0.02, 0.05);
    assert!(four_k > FOUR_KHZ_PRESENT && four_k < third, "4 kHz {four_k:.4} against 8000/6 {third:.4}");

    // 9.2.4.2: "the modem shall terminate transmission of CRe and shall
    // transmit QCA2d followed by silence for 75 +/- 5 ms and then QTS, QTS\
    // and ANSpcm". Measured 75 ms.
    let gap = qts_on - qca2d_off;
    assert!((0.065..=0.085).contains(&gap), "{:.0} ms between QCA2d and QTS", gap * 1000.0);

    // QTS is 768T and QTS\ is 48T, and the join between them is a 180 degree
    // turn of the 8000/6 tone (8.3.6). Measured at 2.117 s, 96 ms after QTS
    // began -- 768 symbols to the millisecond -- with QTS and QTS\ together
    // running 2.021 s to 2.123 s.
    let turns = capture.reversals(F_FRAME_3RD, qts_on, qts_off, TONE_WINDOW);
    println!("QCA2d ends {qca2d_off:.3}, QTS {qts_on:.3}..{qts_off:.3}, QTS\\ from {turns:.3?}");
    assert_eq!(turns.len(), 1, "QTS reversals at {turns:?}");
    let ran = turns[0] - qts_on;
    assert!(
        (QTS_LENGTH - 0.008..=QTS_LENGTH + 0.008).contains(&ran),
        "QTS ran {:.1} ms before QTS\\, not {:.1}",
        ran * 1000.0,
        QTS_LENGTH * 1000.0
    );
    let tail = qts_off - turns[0];
    assert!((0.003..=0.015).contains(&tail), "QTS\\ ran {:.1} ms", tail * 1000.0);
}

#[test]
fn anspcm_reverses_every_3612_symbols_until_toneq_answers_it() {
    let capture = capture();
    // ANSpcm starts as QTS\ ends and runs until the analogue modem's TONEq
    // reaches the far end: 2.121 s to 3.315 s.
    let (on, off) = capture.span(F_ANSPCM, 2.1, 3.5).expect("no ANSpcm");
    assert!(off - on > 1.0, "ANSpcm ran {:.2} s", off - on);

    // 8.3.1: "a phase reversal is added every 3612 symbols", 451.5 ms. Two of
    // them fit in this call, at 2.571 s and 3.026 s. The first is 450 ms
    // after ANSpcm begins, which is the reading section 4 of the plan fixed:
    // the first reversal comes after 3612 symbols, table polarity first.
    let turns = capture.reversals(F_ANSPCM, on, off, 0.010);
    println!("ANSpcm reversals at {turns:.3?}");
    assert_eq!(turns.len(), 2, "ANSpcm reversals at {turns:?}");
    for (name, measured) in [("the first", turns[0] - on), ("the spacing", turns[1] - turns[0])] {
        assert!(
            (ANSPCM_REVERSAL - 0.020..=ANSPCM_REVERSAL + 0.020).contains(&measured),
            "{name}: {:.1} ms, not {:.1}",
            measured * 1000.0,
            ANSPCM_REVERSAL * 1000.0
        );
    }

    // 9.2.1.3: "when ANSpcm has been detected for 1 s the modem shall
    // transmit TONEq for a minimum of 50 ms". The escape clause after it --
    // TONEq as soon as ANSpcm is heard -- needs ANSam to have been detected
    // for a second first, and this call opened with CRe, so the full second
    // is the only path. Measured 999 ms. TONEq is the 980 Hz tone of 8.2.5,
    // which is also the V.21(L) mark, so a V.21(L) demodulator reads it as an
    // endless run of ONEs; that is why the QC hunt above was pointed at the
    // frames rather than at a carrier detector.
    let (toneq_on, toneq_off) = capture.span(F_TONEQ, 3.0, 3.4).expect("no TONEq");
    let wait = toneq_on - on;
    assert!((0.95..=1.05).contains(&wait), "TONEq came {:.0} ms after ANSpcm", wait * 1000.0);
    assert!(toneq_off > off, "TONEq stopped before ANSpcm did");
    assert!(toneq_off - off < 0.100, "TONEq ran {:.0} ms past ANSpcm", (toneq_off - off) * 1000.0);
    assert!(toneq_off - toneq_on > 0.050, "9.2.1.3 asks for TONEq to run at least 50 ms");

    // 9.2.1.3 then asks for 75 +/- 5 ms of silence before Phase 2. This
    // modem takes 93 ms, measuring from where its 980 Hz falls away at
    // 3.327 s to where INFO0a begins -- 49 bits at 600 baud before the
    // 3.502 s at which its CRC checks. Recorded, not asserted as conforming:
    // this is one 2005 modem, not the Recommendation, and the last 35 ms of
    // the tone come back about 15 dB down, which is the line's own echo of it
    // and not something a transmitter can be held to.
    let info0a = sequences()
        .into_iter()
        .find(|(_, side, info)| *side == Side::Answer && matches!(info, Info::Info0(_)))
        .expect("no INFO0a");
    let silence = info0a.0 - 49.0 / 600.0 - toneq_off;
    println!(
        "ANSpcm {on:.3}..{off:.3}, TONEq {toneq_on:.3}..{toneq_off:.3}, then {:.0} ms to INFO0a",
        silence * 1000.0
    );
    assert!((0.070..=0.115).contains(&silence), "{:.0} ms from TONEq to INFO0a", silence * 1000.0);
}

#[test]
fn the_info_sequences_say_whether_v92_and_pcm_upstream_were_used() {
    let found = sequences();
    for (at, side, info) in &found {
        println!("{at:6.3}s {side:?}: {info:?}");
    }
    let kinds: Vec<(Side, &str)> = found
        .iter()
        .map(|(_, side, info)| {
            let kind = match info {
                Info::Info0(_) => "INFO0a",
                Info::Info0d(_) => "INFO0d",
                Info::Info1c(_) => "INFO1d",
                Info::Info1a(_) => "INFO1a (V.34)",
                Info::Info1aPcm(_) => "INFO1a (V.90)",
                Info::Info1aPcmUp(_) => "INFO1a (V.92 PCM upstream)",
                Info::Info1aV34Up(_) => "INFO1a (V.92 Table 19)",
                Info::Mh(_) => "MH",
            };
            (*side, kind)
        })
        .collect();
    // A full Phase 2, and this time INFO0a is readable: in the V.90 recording
    // the server's JM was still going and its 1850 Hz mark sat in INFO0a's
    // band, and here there is no JM to be in the way.
    assert_eq!(
        kinds,
        vec![
            (Side::Answer, "INFO0a"),
            (Side::Call, "INFO0d"),
            (Side::Call, "INFO1d"),
            (Side::Answer, "INFO1a (V.90)"),
        ]
    );

    // Both ends say V.92 (9.3), in the two bits that are the other way round
    // in the two layouts: INFO0a bit 26 and INFO0d bit 27.
    let info0a = found.iter().find_map(|(_, _, i)| if let Info::Info0(x) = i { Some(*x) } else { None }).unwrap();
    let info0d = found.iter().find_map(|(_, _, i)| if let Info::Info0d(x) = i { Some(*x) } else { None }).unwrap();
    assert_eq!(info0a.pcm_flags(), PcmFlags { v92: true, short_phase2: false });
    assert_eq!(info0d.pcm_flags(), PcmFlags { v92: true, short_phase2: true });
    // The server asked for short Phase 2 and the modem did not, so 9.4's
    // "all four" fails and both run the full one -- which is what the four
    // sequences above are.
    assert!(!(info0a.pcm_flags().short_phase2 && info0d.pcm_flags().short_phase2));
    // The same server as the V.90 recording, saying the same things about the
    // network.
    assert!(!info0d.a_law && info0d.power_at_codec);
    assert_eq!(info0d.max_dbm0(), -12.0);
    assert_eq!(info0d.nominal_dbm0(), -10.0);

    // INFO1d bit 70. Both modems have shown V.92 capability, so 8.4.1 and 9.3
    // give the bit V.92's meaning -- "the channel supports PCM upstream" --
    // and it is set. Read as V.90 the same bit would mean 3429 goes up on the
    // high carrier; the V.90 recording of this pair has it clear, so this is
    // not a habit of the server's.
    let info1d = found.iter().find_map(|(_, _, i)| if let Info::Info1c(x) = i { Some(*x) } else { None }).unwrap();
    assert!(info1d.pcm_upstream(), "INFO1d bit 70");
    let rates: Vec<u8> = info1d.probed.iter().map(|p| p.max_rate).collect();
    assert_eq!(rates, vec![5, 6, 6, 7, 7, 9]);
    assert_eq!(info1d.probed_3429().pre_emphasis, 0);

    // And the analogue modem declined it. Bits 37:39 and 34:36 are 6 and 4,
    // which is Table 10/V.90 -- 8000 symbols down, V.34 at 3200 up -- and not
    // Table 18's 6 and 6. A 2005 modem offered PCM upstream by its server did
    // not take it, so there is no Ru, no TRN1u and no CPd anywhere on this
    // recording.
    let asked = found
        .iter()
        .find_map(|(_, _, i)| if let Info::Info1aPcm(x) = i { Some(*x) } else { None })
        .expect("no V.90 INFO1a");
    assert_eq!(asked.upstream, SymbolRate::S3200);
    assert_eq!(asked.uinfo, 78);
    assert_eq!(asked.md_length, 20);
    assert!(
        !found.iter().any(|(_, _, i)| matches!(i, Info::Info1aPcmUp(_))),
        "a Table 18 INFO1a after all"
    );
}

/// Ignored, and the reason is the finding: the analogue modem asked for
/// Table 10/V.90 in its INFO1a, so PCM upstream was never selected and there
/// is nothing after it to hunt for. The hunt is written out anyway, and run
/// against QTS as well, so that it is known to work the day a capture does
/// carry an upstream.
#[test]
#[ignore = "this capture has no PCM upstream; see the module comment"]
fn if_pcm_upstream_was_used_ru_and_trn1u_are_on_the_tap() {
    let capture = capture();
    // Ru repeats {+LU, +LU, +LU, -LU, -LU, -LU} at 8000 symbol/s (8.5.5), so
    // it is a period-6 square wave: energy at 8000/6 and at 4000 Hz, and at
    // nothing else. Both components are required, and for long enough to be a
    // signal rather than a coincidence -- a 3200-baud V.34 carrier does put a
    // stray six milliseconds over the 8000/6 threshold, but it has nothing at
    // 4 kHz to go with it. QTS is the same shape and is the control.
    let period_6 = |from: f64, to: f64| {
        capture
            .run(from, to, |t| {
                let third = capture.tone(F_FRAME_3RD, t, TONE_WINDOW).0;
                let fourth = capture.tone(4000.0, t, TONE_WINDOW).0;
                if fourth > FOUR_KHZ_PRESENT { third } else { 0.0 }
            })
            .filter(|(on, off)| off - on > 0.050)
    };
    assert!(period_6(2.0, 2.2).is_some(), "the detector cannot even find QTS");
    // Phase 3 begins after the 5.121 s INFO1a and would open with Ru, then
    // TRN1u (8.5.6, 8.5.7). Nothing of the sort is there: what follows is
    // V.34's own Phase 3 on a 3200-baud carrier, as V.90 has it.
    assert_eq!(period_6(5.2, 12.0), None, "a period-6 upstream after INFO1a after all");
}
